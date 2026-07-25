use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use regex::Regex;
use yq::v1::eval::{Context, VariableProvider};
use yq::v1::expr::{Atom, Cons, Expression};

use crate::db::models::{DbAccount, DbStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SqlPrefilterValue {
    Text(String),
    Integer(i64),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SqlPrefilter {
    clause: String,
    bindings: Vec<SqlPrefilterValue>,
}

impl SqlPrefilter {
    pub(crate) fn clause(&self) -> &str {
        &self.clause
    }

    pub(crate) fn bindings(&self) -> &[SqlPrefilterValue] {
        &self.bindings
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.clause.is_empty()
    }
}

/// Parsed YQ program plus predicates SQLite can safely use to shrink the
/// candidate set.
///
/// A predicate may be an exact translation or a conservative superset. The
/// evaluator remains authoritative and always applies the original YQ program
/// to every candidate.
pub(crate) struct CompiledQuery {
    expression: Expression,
    sql_prefilter: SqlPrefilter,
}

impl CompiledQuery {
    pub(crate) fn sql_prefilter(&self) -> &SqlPrefilter {
        &self.sql_prefilter
    }
}

/// Reusable evaluation context for one synchronous batch. The separate
/// EvaluationCache can survive across batches in the same query request.
pub(crate) struct Evaluator {
    context: Context,
}

impl Evaluator {
    pub(crate) fn with_cache(cache: EvaluationCache) -> Self {
        let mut context = Context::new();
        register_custom_functions(&mut context, cache);
        Self { context }
    }

    pub(crate) fn matches(
        &mut self,
        query: &CompiledQuery,
        status: &DbStatus,
        account: Option<&DbAccount>,
    ) -> bool {
        self.context
            .set_variable_provider(Box::new(MastodonVariableProvider::new(
                status.clone(),
                account.cloned(),
            )));
        self.context
            .evaluate(&query.expression)
            .map(|result| !result.is_nil())
            .unwrap_or(false)
    }
}

#[derive(Clone, Default)]
pub(crate) struct EvaluationCache {
    regexes: Arc<Mutex<HashMap<String, Option<Regex>>>>,
}

fn html_to_plain_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}

/// VariableProvider that maps Mastodon status/account fields to YQ symbols.
struct MastodonVariableProvider {
    status: DbStatus,
    account: Option<DbAccount>,
    plain_content: String,
}

impl MastodonVariableProvider {
    fn new(status: DbStatus, account: Option<DbAccount>) -> Self {
        let plain_content = html_to_plain_text(&status.content);
        Self {
            status,
            account,
            plain_content,
        }
    }
}

fn bool_to_expr(b: bool) -> Expression {
    if b {
        Expression::t()
    } else {
        Expression::nil()
    }
}

fn opt_str_to_expr(s: &Option<String>) -> Expression {
    match s {
        Some(s) if !s.is_empty() => Expression::Atom(Atom::String(s.clone())),
        _ => Expression::nil(),
    }
}

fn status_application_name(status: &DbStatus) -> Option<String> {
    status
        .application_json
        .as_deref()
        .and_then(|json| {
            serde_json::from_str::<crate::mastodon::types::status::StatusApplication>(json).ok()
        })
        .map(|application| application.name.trim().to_string())
        .filter(|name| !name.is_empty())
}

impl VariableProvider for MastodonVariableProvider {
    fn get(&self, symbol: &str) -> Option<Expression> {
        match symbol {
            // Status text fields
            "text" | "content" => Some(Expression::Atom(Atom::String(self.plain_content.clone()))),
            "raw_content" => Some(Expression::Atom(Atom::String(self.status.content.clone()))),
            "visibility" => Some(Expression::Atom(Atom::String(
                self.status.visibility.clone(),
            ))),
            "language" | "lang" => Some(opt_str_to_expr(&self.status.language)),
            "application" | "application_name" | "source" | "source_app" => {
                Some(opt_str_to_expr(&status_application_name(&self.status)))
            }
            "spoiler_text" | "cw" => {
                if self.status.spoiler_text.is_empty() {
                    Some(Expression::nil())
                } else {
                    Some(Expression::Atom(Atom::String(
                        self.status.spoiler_text.clone(),
                    )))
                }
            }
            "sensitive" => Some(bool_to_expr(self.status.sensitive)),

            // Numeric fields
            "favourites_count" | "fav_count" => Some(Expression::Atom(Atom::Integer(
                self.status.favourites_count,
            ))),
            "reblogs_count" | "boost_count" => {
                Some(Expression::Atom(Atom::Integer(self.status.reblogs_count)))
            }
            "replies_count" => Some(Expression::Atom(Atom::Integer(self.status.replies_count))),

            // Boolean fields
            "bookmarked" => Some(bool_to_expr(self.status.bookmarked.unwrap_or(false))),
            "favourited" | "faved" => Some(bool_to_expr(self.status.favourited.unwrap_or(false))),
            "reblogged" | "boosted" => Some(bool_to_expr(self.status.reblogged.unwrap_or(false))),
            "muted" => Some(bool_to_expr(self.status.muted.unwrap_or(false))),
            "pinned" => Some(bool_to_expr(self.status.pinned.unwrap_or(false))),

            // Derived boolean fields
            "is_reply" => Some(bool_to_expr(self.status.in_reply_to_id.is_some())),
            "is_reblog" | "is_boost" => Some(bool_to_expr(self.status.reblog_of_id.is_some())),
            "has_media" => {
                let has = self
                    .status
                    .media_attachments_json
                    .as_ref()
                    .map(|j| j != "[]" && !j.is_empty())
                    .unwrap_or(false);
                Some(bool_to_expr(has))
            }
            "has_poll" => Some(bool_to_expr(self.status.poll_json.is_some())),
            "has_card" => Some(bool_to_expr(self.status.card_json.is_some())),
            "has_cw" => Some(bool_to_expr(!self.status.spoiler_text.is_empty())),

            // Relation fields
            "in_reply_to_id" => Some(opt_str_to_expr(&self.status.in_reply_to_id)),

            // Account fields
            "user" | "username" => self
                .account
                .as_ref()
                .map(|a| Expression::Atom(Atom::String(a.username.clone()))),
            "acct" => self
                .account
                .as_ref()
                .map(|a| Expression::Atom(Atom::String(a.acct.clone()))),
            "display_name" => self
                .account
                .as_ref()
                .map(|a| Expression::Atom(Atom::String(a.display_name.clone()))),
            "bot" => self.account.as_ref().map(|a| bool_to_expr(a.bot)),
            "locked" => self.account.as_ref().map(|a| bool_to_expr(a.locked)),

            // Server/domain
            "server_domain" | "domain" => Some(Expression::Atom(Atom::String(
                self.status.server_domain.clone(),
            ))),

            _ => None,
        }
    }
}

/// Register custom functions (e.g., regex) into a YQ Context.
fn register_custom_functions(ctx: &mut Context, cache: EvaluationCache) {
    ctx.register_function("regex", move |context, _symbol, cdr| {
        let mut iter = cdr.iter();
        let haystack =
            context.evaluate(iter.next().ok_or_else(error_wrong_number_of_arguments)?)?;
        let pattern = context.evaluate(iter.next().ok_or_else(error_wrong_number_of_arguments)?)?;

        match (haystack, pattern) {
            (
                Expression::Atom(Atom::String(h)) | Expression::Atom(Atom::Symbol(h)),
                Expression::Atom(Atom::String(p)) | Expression::Atom(Atom::Symbol(p)),
            ) => {
                let Ok(mut cache_map) = cache.regexes.lock() else {
                    return Ok(Expression::nil());
                };
                let cache_key = p.clone();
                let re = cache_map
                    .entry(cache_key)
                    .or_insert_with(|| Regex::new(&p).ok());
                if re.as_ref().is_some_and(|regex| regex.is_match(&h)) {
                    Ok(Expression::t())
                } else {
                    Ok(Expression::nil())
                }
            }
            _ => Ok(Expression::nil()),
        }
    });
}

fn error_wrong_number_of_arguments() -> Expression {
    Expression::Cons(Cons::from(
        Atom::symbol("wrong-number-of-arguments").into(),
        Expression::nil(),
    ))
}

/// Normalize a YQ query string so the parser accepts it.
/// If the query doesn't start with "from" or "where", prepend "where ".
fn normalize_query(query_str: &str) -> String {
    let trimmed = query_str.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    if first_word.eq_ignore_ascii_case("from") || first_word.eq_ignore_ascii_case("where") {
        trimmed.to_string()
    } else {
        format!("where {}", trimmed)
    }
}

/// Parse a YQ query string and return the filter expression.
/// Returns the cloned Expression since Query type is not publicly accessible.
pub fn parse_expression(query_str: &str) -> Result<Expression, String> {
    let normalized = normalize_query(query_str);
    let query = yq::v1::parser::parse(&normalized).map_err(|e| format!("YQ parse error: {}", e))?;
    Ok(query.expression().clone())
}

pub(crate) fn compile_query(query_str: &str) -> Result<CompiledQuery, String> {
    let expression = parse_expression(query_str)?;
    let sql_prefilter = build_sql_prefilter(&expression);
    Ok(CompiledQuery {
        expression,
        sql_prefilter,
    })
}

#[derive(Debug, Clone, Default)]
struct SqlPredicate {
    clause: String,
    bindings: Vec<SqlPrefilterValue>,
}

fn build_sql_prefilter(expression: &Expression) -> SqlPrefilter {
    let mut predicates = Vec::new();
    collect_safe_conjuncts(expression, &mut predicates);
    if predicates.is_empty() {
        return SqlPrefilter::default();
    }

    let clause = predicates
        .iter()
        .map(|predicate| format!("({})", predicate.clause))
        .collect::<Vec<_>>()
        .join(" AND ");
    let bindings = predicates
        .into_iter()
        .flat_map(|predicate| predicate.bindings)
        .collect();
    SqlPrefilter { clause, bindings }
}

fn collect_safe_conjuncts(expression: &Expression, predicates: &mut Vec<SqlPredicate>) {
    let items = expression.iter().collect::<Vec<_>>();
    if items.first().and_then(|item| expression_symbol(item)) == Some("and") {
        for item in items.into_iter().skip(1) {
            collect_safe_conjuncts(item, predicates);
        }
        return;
    }

    if let Some(predicate) = translate_safe_predicate(expression) {
        predicates.push(predicate);
    }
}

fn translate_safe_predicate(expression: &Expression) -> Option<SqlPredicate> {
    if let Some(symbol) = expression_symbol(expression) {
        return boolean_predicate(symbol);
    }

    let items = expression.iter().collect::<Vec<_>>();
    let operator = items.first().and_then(|item| expression_symbol(item))?;
    match operator {
        "and" | "&" => combine_predicates("AND", &items[1..]),
        "or" | "|" => combine_predicates("OR", &items[1..]),
        // Avoid SQL NOT pushdown: YQ treats a missing variable as an evaluation
        // error while SQL uses three-valued NULL logic, so negation could turn
        // a safe candidate filter into a false negative.
        "not" | "!" => None,
        "equals" | "eq" | "=" | "==" if items.len() == 3 => equality_predicate(items[1], items[2])
            .or_else(|| equality_predicate(items[2], items[1])),
        "contains" | "in" if items.len() == 3 => contains_predicate(items[1], items[2]),
        _ => None,
    }
}

fn combine_predicates(operator: &str, expressions: &[&Expression]) -> Option<SqlPredicate> {
    if expressions.is_empty() {
        return None;
    }
    let predicates = expressions
        .iter()
        .map(|expression| translate_safe_predicate(expression))
        .collect::<Option<Vec<_>>>()?;
    let clause = predicates
        .iter()
        .map(|predicate| format!("({})", predicate.clause))
        .collect::<Vec<_>>()
        .join(&format!(" {operator} "));
    let bindings = predicates
        .into_iter()
        .flat_map(|predicate| predicate.bindings)
        .collect();
    Some(SqlPredicate { clause, bindings })
}

fn contains_predicate(haystack: &Expression, needle: &Expression) -> Option<SqlPredicate> {
    let symbol = expression_symbol(haystack)?;
    let Expression::Atom(Atom::String(needle)) = needle else {
        return None;
    };
    if !matches!(symbol, "text" | "content") {
        return None;
    }

    let pattern = raw_html_subsequence_like_pattern(needle)?;
    Some(SqlPredicate {
        clause: "s.content LIKE ? ESCAPE '\\'".to_string(),
        bindings: vec![SqlPrefilterValue::Text(pattern)],
    })
}

/// Build a conservative SQL candidate predicate for a substring of rendered
/// status text.
///
/// `text`/`content` removes HTML tags and decodes a small set of entities.
/// Therefore a visible substring is not necessarily contiguous in the stored
/// HTML. Every character other than one introduced by those entity decodes is,
/// however, present in the source in the same order. A `%`-separated LIKE
/// pattern over the first bounded set of those characters is consequently a
/// superset of the YQ result: it admits false positives for the authoritative
/// evaluator, but cannot omit a real match.
fn raw_html_subsequence_like_pattern(needle: &str) -> Option<String> {
    const MAX_ANCHORS: usize = 64;

    if needle.contains('\0') {
        return None;
    }

    let mut pattern = String::with_capacity(2 + MAX_ANCHORS * 6);
    pattern.push('%');
    let mut anchor_count = 0usize;
    for character in needle.chars() {
        // These characters may have been introduced by html_to_plain_text's
        // entity decoding and thus need not occur literally in the raw HTML.
        if matches!(character, ' ' | '&' | '<' | '>' | '"') {
            continue;
        }
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
        pattern.push('%');
        anchor_count += 1;
        if anchor_count >= MAX_ANCHORS {
            break;
        }
    }
    (anchor_count > 0).then_some(pattern)
}

fn equality_predicate(variable: &Expression, value: &Expression) -> Option<SqlPredicate> {
    let symbol = expression_symbol(variable)?;
    match value {
        Expression::Atom(Atom::String(value)) => string_equality_predicate(symbol, value),
        Expression::Atom(Atom::Integer(value)) => integer_equality_predicate(symbol, *value),
        _ => None,
    }
}

fn string_equality_predicate(symbol: &str, value: &str) -> Option<SqlPredicate> {
    let clause = match symbol {
        "visibility" => "s.visibility = ?",
        "language" | "lang" => "s.language = ?",
        "server_domain" | "domain" => "s.server_domain = ?",
        "in_reply_to_id" => "s.in_reply_to_id = ?",
        // The provider maps an empty CW to nil, so equality with an empty
        // string cannot be pushed down without changing YQ semantics.
        "spoiler_text" | "cw" if !value.is_empty() => "s.spoiler_text = ?",
        "user" | "username" => {
            "EXISTS (SELECT 1 FROM accounts ya WHERE ya.id = s.account_id AND ya.server_domain = s.server_domain AND ya.username = ?)"
        }
        "acct" => {
            "EXISTS (SELECT 1 FROM accounts ya WHERE ya.id = s.account_id AND ya.server_domain = s.server_domain AND ya.acct = ?)"
        }
        "display_name" => {
            "EXISTS (SELECT 1 FROM accounts ya WHERE ya.id = s.account_id AND ya.server_domain = s.server_domain AND ya.display_name = ?)"
        }
        _ => return None,
    };
    Some(SqlPredicate {
        clause: clause.to_string(),
        bindings: vec![SqlPrefilterValue::Text(value.to_string())],
    })
}

fn integer_equality_predicate(symbol: &str, value: i64) -> Option<SqlPredicate> {
    let column = match symbol {
        "favourites_count" | "fav_count" => "s.favourites_count",
        "reblogs_count" | "boost_count" => "s.reblogs_count",
        "replies_count" => "s.replies_count",
        _ => return None,
    };
    Some(SqlPredicate {
        clause: format!("{column} = ?"),
        bindings: vec![SqlPrefilterValue::Integer(value)],
    })
}

fn boolean_predicate(symbol: &str) -> Option<SqlPredicate> {
    let clause = match symbol {
        "sensitive" => "s.sensitive != 0",
        "bookmarked" => "COALESCE(s.bookmarked, 0) != 0",
        "favourited" | "faved" => "COALESCE(s.favourited, 0) != 0",
        "reblogged" | "boosted" => "COALESCE(s.reblogged, 0) != 0",
        "muted" => "COALESCE(s.muted, 0) != 0",
        "pinned" => "COALESCE(s.pinned, 0) != 0",
        "is_reply" => "s.in_reply_to_id IS NOT NULL",
        "is_reblog" | "is_boost" => "s.reblog_of_id IS NOT NULL",
        "has_media" => {
            "s.media_attachments_json IS NOT NULL AND s.media_attachments_json != '' AND s.media_attachments_json != '[]'"
        }
        "has_poll" => "s.poll_json IS NOT NULL",
        "has_card" => "s.card_json IS NOT NULL",
        "has_cw" => "s.spoiler_text != ''",
        "bot" => {
            "EXISTS (SELECT 1 FROM accounts ya WHERE ya.id = s.account_id AND ya.server_domain = s.server_domain AND ya.bot != 0)"
        }
        "locked" => {
            "EXISTS (SELECT 1 FROM accounts ya WHERE ya.id = s.account_id AND ya.server_domain = s.server_domain AND ya.locked != 0)"
        }
        _ => return None,
    };
    Some(SqlPredicate {
        clause: clause.to_string(),
        bindings: Vec::new(),
    })
}

fn expression_symbol(expression: &Expression) -> Option<&str> {
    match expression {
        Expression::Atom(Atom::Symbol(symbol)) => Some(symbol.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_pushes_only_exact_safe_conjuncts_to_sql() {
        let query = compile_query(
            "where (and (= visibility \"private\") (= domain \"example.test\") (regex text \"needle\"))",
        )
        .unwrap();

        assert!(query.sql_prefilter().clause().contains("s.visibility = ?"));
        assert!(query
            .sql_prefilter()
            .clause()
            .contains("s.server_domain = ?"));
        assert_eq!(
            query.sql_prefilter().bindings(),
            &[
                SqlPrefilterValue::Text("private".to_string()),
                SqlPrefilterValue::Text("example.test".to_string()),
            ]
        );
    }

    #[test]
    fn compiler_does_not_push_one_branch_of_an_or_expression() {
        let query =
            compile_query("where (or (= visibility \"private\") (regex text \"needle\"))").unwrap();

        assert!(query.sql_prefilter().is_empty());
    }

    #[test]
    fn compiler_pushes_contains_or_as_a_safe_html_subsequence() {
        let query = compile_query(
            "where (or (contains text \"#えあいさん\") (contains content \"100%_safe\"))",
        )
        .unwrap();

        assert_eq!(
            query.sql_prefilter().clause(),
            "((s.content LIKE ? ESCAPE '\\') OR (s.content LIKE ? ESCAPE '\\'))"
        );
        assert_eq!(
            query.sql_prefilter().bindings(),
            &[
                SqlPrefilterValue::Text("%#%え%あ%い%さ%ん%".to_string()),
                SqlPrefilterValue::Text("%1%0%0%\\%%\\_%s%a%f%e%".to_string()),
            ]
        );
    }

    #[test]
    fn html_subsequence_prefilter_omits_entity_decoded_characters() {
        assert_eq!(
            raw_html_subsequence_like_pattern("fish & <chips>\""),
            Some("%f%i%s%h%c%h%i%p%s%".to_string())
        );
        assert_eq!(raw_html_subsequence_like_pattern(" &<>\" "), None);
    }
}
