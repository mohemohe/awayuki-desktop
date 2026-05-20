use std::cell::RefCell;
use std::collections::HashMap;

use regex::Regex;
use yq::v1::eval::{Context, VariableProvider};
use yq::v1::expr::{Atom, Cons, Expression};

use crate::db::models::{DbAccount, DbStatus};

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

/// Create a YQ evaluation context with custom functions and the given status as variable source.
fn create_context(status: DbStatus, account: Option<DbAccount>) -> Context {
    let provider = MastodonVariableProvider::new(status, account);
    let mut context = Context::new();
    context.set_variable_provider(Box::new(provider));
    register_custom_functions(&mut context);
    context
}

/// Register custom functions (e.g., regex) into a YQ Context.
fn register_custom_functions(ctx: &mut Context) {
    let cache: RefCell<HashMap<String, Regex>> = RefCell::new(HashMap::new());

    ctx.register_function("regex", move |context, _symbol, cdr| {
        let mut iter = cdr.iter();
        let haystack = context.evaluate(
            iter.next()
                .ok_or_else(|| error_wrong_number_of_arguments())?,
        )?;
        let pattern = context.evaluate(
            iter.next()
                .ok_or_else(|| error_wrong_number_of_arguments())?,
        )?;

        match (haystack, pattern) {
            (
                Expression::Atom(Atom::String(h)) | Expression::Atom(Atom::Symbol(h)),
                Expression::Atom(Atom::String(p)) | Expression::Atom(Atom::Symbol(p)),
            ) => {
                let mut cache_map = cache.borrow_mut();
                let re = if let Some(re) = cache_map.get(&p) {
                    re
                } else {
                    match Regex::new(&p) {
                        Ok(re) => {
                            cache_map.insert(p.clone(), re);
                            cache_map.get(&p).unwrap()
                        }
                        Err(_) => return Ok(Expression::nil()),
                    }
                };
                if re.is_match(&h) {
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

/// Filter a list of statuses using a YQ query string.
pub fn filter_statuses(
    query_str: &str,
    statuses: Vec<(DbStatus, Option<DbAccount>)>,
) -> Result<Vec<(DbStatus, Option<DbAccount>)>, String> {
    let expression = parse_expression(query_str)?;

    let mut results = Vec::new();
    for (status, account) in statuses {
        let mut context = create_context(status.clone(), account.clone());
        match context.evaluate(&expression) {
            Ok(result) => {
                if !result.is_nil() {
                    results.push((status, account));
                }
            }
            Err(e) => {
                tracing::debug!("YQ eval error for status {}: {:?}", status.id, e);
            }
        }
    }
    Ok(results)
}

/// Check if a single status matches an already-parsed YQ expression.
pub fn matches_expression(
    expression: &Expression,
    status: &DbStatus,
    account: Option<&DbAccount>,
) -> bool {
    let mut context = create_context(status.clone(), account.cloned());
    context
        .evaluate(expression)
        .map(|r| !r.is_nil())
        .unwrap_or(false)
}

/// Check if a single status matches a YQ query.
pub fn matches_status(query_str: &str, status: &DbStatus, account: Option<&DbAccount>) -> bool {
    let Ok(expression) = parse_expression(query_str) else {
        return false;
    };

    matches_expression(&expression, status, account)
}
