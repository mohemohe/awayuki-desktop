//! Connection-local SQLite extensions required by portable search migrations.
//!
//! FTS5 tokenizer registrations belong to an individual `sqlite3*`, not to
//! the database file. The pool therefore installs this tokenizer from its
//! `after_connect` hook on the writer and on every lazily opened WAL reader.
//! The same hook exposes `awayuki_icu_match` for bounded pending/backfill-gap
//! windows and `awayuki_icu_index_match` for cheap matching of a bounded recent
//! encoded-token window.

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use libsqlite3_sys::{
    fts5_api, fts5_tokenizer, sqlite3, sqlite3_bind_pointer, sqlite3_context,
    sqlite3_create_function_v2, sqlite3_finalize, sqlite3_prepare_v2, sqlite3_result_error,
    sqlite3_result_int, sqlite3_step, sqlite3_stmt, sqlite3_value, sqlite3_value_bytes,
    sqlite3_value_text, Fts5Tokenizer, FTS5_TOKENIZE_QUERY, FTS5_TOKEN_COLOCATED,
    SQLITE_DETERMINISTIC, SQLITE_ERROR, SQLITE_MISUSE, SQLITE_OK, SQLITE_ROW, SQLITE_UTF8,
};
use sqlx::sqlite::SqliteConnection;

pub const TOKENIZER_NAME: &str = "awayuki_short";

const FTS5_API_QUERY: &[u8] = b"SELECT fts5(?1)\0";
const FTS5_API_POINTER_TYPE: &[u8] = b"fts5_api_ptr\0";
const TOKENIZER_NAME_C: &[u8] = b"awayuki_short\0";
const ICU_MATCH_FUNCTION_NAME_C: &[u8] = b"awayuki_icu_match\0";
const ICU_MATCH_ERROR_C: &[u8] = b"awayuki_icu_match received invalid UTF-8\0";
const ICU_INDEX_MATCH_FUNCTION_NAME_C: &[u8] = b"awayuki_icu_index_match\0";
const ICU_INDEX_MATCH_ERROR_C: &[u8] = b"awayuki_icu_index_match received invalid UTF-8\0";

/// Register the short-search tokenizer on one SQLx connection.
pub async fn register(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut handle = connection.lock_handle().await?;
    let database = handle.as_raw_handle().as_ptr();
    // SAFETY: `lock_handle` gives exclusive access to this live sqlite3
    // handle for the duration of the registration call.
    let result = unsafe {
        let tokenizer_result = register_raw(database);
        if tokenizer_result == SQLITE_OK {
            register_icu_match_functions(database)
        } else {
            tokenizer_result
        }
    };
    if result == SQLITE_OK {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "failed to register FTS5 tokenizer {TOKENIZER_NAME}: SQLite error {result}"
        )))
    }
}

unsafe fn register_icu_match_functions(database: *mut sqlite3) -> c_int {
    let match_result = sqlite3_create_function_v2(
        database,
        ICU_MATCH_FUNCTION_NAME_C.as_ptr().cast(),
        -1,
        SQLITE_UTF8 | SQLITE_DETERMINISTIC,
        ptr::null_mut(),
        Some(icu_match),
        None,
        None,
        None,
    );
    if match_result != SQLITE_OK {
        return match_result;
    }
    sqlite3_create_function_v2(
        database,
        ICU_INDEX_MATCH_FUNCTION_NAME_C.as_ptr().cast(),
        -1,
        SQLITE_UTF8 | SQLITE_DETERMINISTIC,
        ptr::null_mut(),
        Some(icu_index_match),
        None,
        None,
        None,
    )
}

unsafe extern "C" fn icu_index_match(
    context: *mut sqlite3_context,
    argument_count: c_int,
    arguments: *mut *mut sqlite3_value,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if argument_count != 3 || arguments.is_null() {
            return None;
        }
        let arguments = std::slice::from_raw_parts(arguments, argument_count as usize);
        let query_token_text = sqlite_value_str(arguments[0])?;
        let indexed_fields = [
            sqlite_value_str(arguments[1])?,
            sqlite_value_str(arguments[2])?,
        ];
        Some(crate::db::icu_search::matches_index_text(
            query_token_text,
            &indexed_fields,
        ))
    }));
    match result {
        Ok(Some(matched)) => sqlite3_result_int(context, c_int::from(matched)),
        Ok(None) | Err(_) => {
            sqlite3_result_error(context, ICU_INDEX_MATCH_ERROR_C.as_ptr().cast(), -1)
        }
    }
}

unsafe extern "C" fn icu_match(
    context: *mut sqlite3_context,
    argument_count: c_int,
    arguments: *mut *mut sqlite3_value,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if argument_count < 2 || arguments.is_null() {
            return None;
        }
        let arguments = std::slice::from_raw_parts(arguments, argument_count as usize);
        let term = sqlite_value_str(arguments[0])?;
        let fields = arguments[1..]
            .iter()
            .map(|value| sqlite_value_str(*value))
            .collect::<Option<Vec<_>>>()?;
        Some(crate::db::icu_search::matches_fields(term, fields))
    }));
    match result {
        Ok(Some(matched)) => sqlite3_result_int(context, c_int::from(matched)),
        Ok(None) | Err(_) => sqlite3_result_error(context, ICU_MATCH_ERROR_C.as_ptr().cast(), -1),
    }
}

unsafe fn sqlite_value_str<'a>(value: *mut sqlite3_value) -> Option<&'a str> {
    if value.is_null() {
        return Some("");
    }
    let length = sqlite3_value_bytes(value);
    let text = sqlite3_value_text(value);
    if length == 0 {
        return Some("");
    }
    if length < 0 || text.is_null() {
        return None;
    }
    std::str::from_utf8(std::slice::from_raw_parts(text, length as usize)).ok()
}

unsafe fn register_raw(database: *mut sqlite3) -> c_int {
    let api = fts5_api_from_database(database);
    if api.is_null() {
        return SQLITE_ERROR;
    }
    let Some(create_tokenizer) = (*api).xCreateTokenizer else {
        return SQLITE_ERROR;
    };
    let mut tokenizer = fts5_tokenizer {
        xCreate: Some(tokenizer_create),
        xDelete: Some(tokenizer_delete),
        xTokenize: Some(tokenize),
    };
    create_tokenizer(
        api,
        TOKENIZER_NAME_C.as_ptr().cast(),
        ptr::null_mut(),
        &mut tokenizer,
        None,
    )
}

unsafe fn fts5_api_from_database(database: *mut sqlite3) -> *mut fts5_api {
    let mut statement: *mut sqlite3_stmt = ptr::null_mut();
    let prepared = sqlite3_prepare_v2(
        database,
        FTS5_API_QUERY.as_ptr().cast(),
        -1,
        &mut statement,
        ptr::null_mut(),
    );
    if prepared != SQLITE_OK || statement.is_null() {
        return ptr::null_mut();
    }

    let mut api: *mut fts5_api = ptr::null_mut();
    let bound = sqlite3_bind_pointer(
        statement,
        1,
        (&mut api as *mut *mut fts5_api).cast(),
        FTS5_API_POINTER_TYPE.as_ptr().cast(),
        None,
    );
    let stepped = if bound == SQLITE_OK {
        sqlite3_step(statement)
    } else {
        bound
    };
    let finalized = sqlite3_finalize(statement);
    if stepped == SQLITE_ROW && finalized == SQLITE_OK {
        api
    } else {
        ptr::null_mut()
    }
}

unsafe extern "C" fn tokenizer_create(
    _context: *mut c_void,
    _arguments: *mut *const c_char,
    _argument_count: c_int,
    output: *mut *mut Fts5Tokenizer,
) -> c_int {
    if output.is_null() {
        return SQLITE_MISUSE;
    }
    // The tokenizer has no per-instance state, but FTS5 requires a distinct
    // non-null handle that xDelete can release exactly once.
    *output = Box::into_raw(Box::new(0_u8)).cast();
    SQLITE_OK
}

unsafe extern "C" fn tokenizer_delete(tokenizer: *mut Fts5Tokenizer) {
    if !tokenizer.is_null() {
        drop(Box::from_raw(tokenizer.cast::<u8>()));
    }
}

type TokenCallback = unsafe extern "C" fn(
    context: *mut c_void,
    flags: c_int,
    token: *const c_char,
    token_len: c_int,
    start: c_int,
    end: c_int,
) -> c_int;

unsafe extern "C" fn tokenize(
    _tokenizer: *mut Fts5Tokenizer,
    context: *mut c_void,
    flags: c_int,
    text: *const c_char,
    text_len: c_int,
    callback: Option<TokenCallback>,
) -> c_int {
    if text_len < 0 || (text.is_null() && text_len != 0) {
        return SQLITE_MISUSE;
    }
    let Some(callback) = callback else {
        return SQLITE_MISUSE;
    };
    let bytes = if text_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(text.cast::<u8>(), text_len as usize)
    };
    let Ok(value) = std::str::from_utf8(bytes) else {
        return SQLITE_ERROR;
    };

    // Search code binds a pre-encoded unigram or bigram marker. Treat that
    // marker as one literal token instead of recursively making grams from it.
    if flags & FTS5_TOKENIZE_QUERY != 0 && is_encoded_query_token(bytes) {
        return callback(context, 0, text, text_len, 0, text_len);
    }

    let mut characters = value.char_indices().peekable();
    while let Some((start, character)) = characters.next() {
        // Search input is split on whitespace and cannot contain NUL field
        // separators. Omitting those dead postings also prevents bigrams from
        // crossing field/word boundaries.
        if character.is_whitespace() || character.is_control() {
            continue;
        }
        let end = start.saturating_add(character.len_utf8());
        let mut unigram = [0_u8; 7];
        unigram[0] = b'u';
        write_codepoint(&mut unigram[1..], normalize_codepoint(character));
        let result = callback(
            context,
            0,
            unigram.as_ptr().cast(),
            unigram.len() as c_int,
            usize_to_c_int(start),
            usize_to_c_int(end),
        );
        if result != SQLITE_OK {
            return result;
        }

        if let Some((next_start, next_character)) = characters
            .peek()
            .copied()
            .filter(|(_, character)| !character.is_whitespace() && !character.is_control())
        {
            let mut bigram = [0_u8; 13];
            bigram[0] = b'b';
            write_codepoint(&mut bigram[1..7], normalize_codepoint(character));
            write_codepoint(&mut bigram[7..], normalize_codepoint(next_character));
            let next_end = next_start.saturating_add(next_character.len_utf8());
            let result = callback(
                context,
                FTS5_TOKEN_COLOCATED,
                bigram.as_ptr().cast(),
                bigram.len() as c_int,
                usize_to_c_int(start),
                usize_to_c_int(next_end),
            );
            if result != SQLITE_OK {
                return result;
            }
        }
    }
    SQLITE_OK
}

fn normalize_codepoint(character: char) -> u32 {
    if character.is_ascii_uppercase() {
        character.to_ascii_lowercase() as u32
    } else {
        character as u32
    }
}

fn write_codepoint(output: &mut [u8], codepoint: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let output_len = output.len();
    for (index, byte) in output.iter_mut().enumerate() {
        let shift = (output_len - index - 1) * 4;
        *byte = HEX[((codepoint >> shift) & 0x0f) as usize];
    }
}

fn is_encoded_query_token(value: &[u8]) -> bool {
    let expected_len = match value.first() {
        Some(b'u') => 7,
        Some(b'b') => 13,
        _ => return false,
    };
    value.len() == expected_len
        && value[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn usize_to_c_int(value: usize) -> c_int {
    c_int::try_from(value).unwrap_or(c_int::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn connection_registration_exposes_the_icu_matcher() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| Box::pin(register(connection)))
            .connect("sqlite::memory:")
            .await
            .expect("open ICU matcher fixture");
        let matched =
            sqlx::query_scalar::<_, bool>("SELECT awayuki_icu_match('STRASSE', 'Straße')")
                .fetch_one(&pool)
                .await
                .expect("call ICU matcher");
        assert!(matched);
        let indexed = crate::db::icu_search::index_text(["Straße"]);
        let query = crate::db::icu_search::index_text(["STRASS"]);
        let indexed_match =
            sqlx::query_scalar::<_, bool>("SELECT awayuki_icu_index_match(?, ?, '')")
                .bind(query)
                .bind(indexed)
                .fetch_one(&pool)
                .await
                .expect("call encoded ICU matcher");
        assert!(indexed_match);
    }
}
