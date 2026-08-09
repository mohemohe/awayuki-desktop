// SPDX-License-Identifier: MIT
//
// KQ evaluation semantics are derived from Krile StarryEyes
// (Copyright (c) 2013 Karno and StarryEyes contributors, MIT License),
// revision a2c4c9b68287c9058d82a15cd28c6615863a626f. This is an
// idiomatic, provider-neutral implementation over Awayuki's cached models.

use std::collections::{HashMap, HashSet};

use icu_casemap::CaseMapper;
use icu_normalizer::ComposingNormalizerBorrowed;
use regex::{Regex, RegexBuilder};

use crate::db::models::{DbAccount, DbStatus, DbStatusViewerState};
use crate::mastodon::types::status::{
    Card, MediaAttachment, Mention, Poll, StatusApplication, Tag,
};

use super::kq_filter::{
    BinaryOp, EvaluationContext, Expr, ExprKind, Field, SourceKind, SourceSpec, StatusKey, UnaryOp,
};

const MAX_REGEX_BYTES: usize = 4 * 1024;
const MAX_REGEX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_REGEX_CACHE_ENTRIES: usize = 64;
const MAX_DERIVED_TEXT_BYTES: usize = 1024 * 1024;
const MAX_JSON_BYTES: usize = 1024 * 1024;

/// Kleene's strong three-valued Boolean used for values that are not present
/// in a provider response or cannot be safely resolved from the local cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
enum Value {
    Unknown,
    Bool(Truth),
    Number(i64),
    Text { value: String, caseful: bool },
    Identity(String),
    Set(Vec<ScalarValue>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ScalarValue {
    Number(i64),
    Text(String),
    Identity(String),
}

impl Value {
    fn text(value: impl Into<String>) -> Self {
        Self::Text {
            value: value.into(),
            caseful: false,
        }
    }

    fn boolean(value: bool) -> Self {
        Self::Bool(if value { Truth::True } else { Truth::False })
    }

    fn truth(&self) -> Truth {
        match self {
            Self::Bool(truth) => *truth,
            _ => Truth::Unknown,
        }
    }

    fn into_scalar(self) -> Option<ScalarValue> {
        match self {
            Self::Number(value) => Some(ScalarValue::Number(value)),
            Self::Text { value, .. } => Some(ScalarValue::Text(value)),
            Self::Identity(value) => Some(ScalarValue::Identity(value)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct SourceBranch {
    viewer_account_acct: Option<String>,
}

/// Stateful only for the bounded dynamic-regex cache. One evaluator is used
/// for a page scan, so invalid dynamic patterns are not recompiled per row.
#[derive(Debug, Default)]
pub(crate) struct Evaluator {
    dynamic_regexes: HashMap<String, Option<Regex>>,
}

impl Evaluator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Match one cached row. Every source argument is evaluated as its own
    /// branch so viewer state from two login accounts is never aggregated.
    pub(crate) fn matches(
        &mut self,
        sources: &[SourceSpec],
        predicate: &Expr,
        context: &EvaluationContext<'_>,
    ) -> bool {
        for source in sources {
            for branch in source_branches(source, context) {
                if self.eval(predicate, context, &branch).truth() == Truth::True {
                    return true;
                }
            }
        }
        false
    }

    fn eval(
        &mut self,
        expression: &Expr,
        context: &EvaluationContext<'_>,
        branch: &SourceBranch,
    ) -> Value {
        match &expression.kind {
            ExprKind::Bool(value) => Value::boolean(*value),
            ExprKind::Number(value) => Value::Number(*value),
            ExprKind::Text(value) => Value::text(value.clone()),
            ExprKind::Identity(value) => Value::Identity(value.clone()),
            ExprKind::Set(items) => eval_set(items, context, branch, self),
            ExprKind::Field(field) => field_value(*field, context, branch),
            ExprKind::Unary(operator, operand) => {
                let value = self.eval(operand, context, branch);
                eval_unary(*operator, value)
            }
            ExprKind::Binary {
                op,
                left,
                right,
                literal_regex,
            } => self.eval_binary(*op, left, right, literal_regex.as_ref(), context, branch),
        }
    }

    fn eval_binary(
        &mut self,
        operator: BinaryOp,
        left: &Expr,
        right: &Expr,
        literal_regex: Option<&Regex>,
        context: &EvaluationContext<'_>,
        branch: &SourceBranch,
    ) -> Value {
        // Preserve Kleene short-circuiting. Besides avoiding unnecessary JSON
        // work, this prevents an unknown right side from weakening `false & x`
        // or `true | x`.
        if operator == BinaryOp::And {
            let left = self.eval(left, context, branch).truth();
            if left == Truth::False {
                return Value::Bool(Truth::False);
            }
            let right = self.eval(right, context, branch).truth();
            return Value::Bool(left.and(right));
        }
        if operator == BinaryOp::Or {
            let left = self.eval(left, context, branch).truth();
            if left == Truth::True {
                return Value::Bool(Truth::True);
            }
            let right = self.eval(right, context, branch).truth();
            return Value::Bool(left.or(right));
        }

        let left = self.eval(left, context, branch);
        let right = self.eval(right, context, branch);
        match operator {
            BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
            BinaryOp::Equal => Value::Bool(equal_values(&left, &right)),
            BinaryOp::NotEqual => Value::Bool(equal_values(&left, &right).not()),
            BinaryOp::Less => compare_numbers(left, right, |a, b| a < b),
            BinaryOp::LessEqual => compare_numbers(left, right, |a, b| a <= b),
            BinaryOp::Greater => compare_numbers(left, right, |a, b| a > b),
            BinaryOp::GreaterEqual => compare_numbers(left, right, |a, b| a >= b),
            BinaryOp::StartsWith => {
                compare_text(left, right, |haystack, needle| haystack.starts_with(needle))
            }
            BinaryOp::EndsWith => {
                compare_text(left, right, |haystack, needle| haystack.ends_with(needle))
            }
            BinaryOp::Contains => contains(left, right),
            BinaryOp::In => is_in(left, right),
            BinaryOp::Regex => self.regex(left, right, literal_regex),
            BinaryOp::Add => add(left, right),
            BinaryOp::Subtract => subtract(left, right),
            BinaryOp::Multiply => multiply(left, right),
            BinaryOp::Divide => divide(left, right),
        }
    }

    fn regex(&mut self, input: Value, pattern: Value, literal_regex: Option<&Regex>) -> Value {
        let Some((input, _)) = text_parts(&input) else {
            return Value::Bool(Truth::Unknown);
        };
        let Some((pattern, _)) = text_parts(&pattern) else {
            return Value::Bool(Truth::Unknown);
        };
        if input.len() > MAX_REGEX_INPUT_BYTES || pattern.len() > MAX_REGEX_BYTES {
            return Value::Bool(Truth::Unknown);
        }
        if let Some(regex) = literal_regex {
            return Value::boolean(regex.is_match(input));
        }
        if !self.dynamic_regexes.contains_key(pattern) {
            if self.dynamic_regexes.len() >= MAX_REGEX_CACHE_ENTRIES {
                self.dynamic_regexes.clear();
            }
            let compiled = RegexBuilder::new(pattern)
                .size_limit(MAX_REGEX_INPUT_BYTES)
                .build()
                .ok();
            self.dynamic_regexes.insert(pattern.to_string(), compiled);
        }
        self.dynamic_regexes
            .get(pattern)
            .and_then(Option::as_ref)
            .map(|regex| Value::boolean(regex.is_match(input)))
            .unwrap_or(Value::Bool(Truth::Unknown))
    }
}

fn source_branches(source: &SourceSpec, context: &EvaluationContext<'_>) -> Vec<SourceBranch> {
    match source.kind {
        SourceKind::Local => unscoped_branch(source.arguments.is_empty()),
        SourceKind::Search => text_source_branches(source, context, true),
        SourceKind::Track => text_source_branches(source, context, false),
        SourceKind::Conversation => unscoped_branch(conversation_source_matches(context)),
        SourceKind::User => source
            .arguments
            .iter()
            .filter(|selector| author_matches_selector(context, selector))
            .map(|_| SourceBranch {
                viewer_account_acct: None,
            })
            .collect(),
        SourceKind::Hashtag => source
            .arguments
            .iter()
            .flat_map(|tag| hashtag_source_branches(clean_tag(tag), context))
            .collect(),
        SourceKind::List => list_source_branches(source, context),
        SourceKind::Home
        | SourceKind::Mentions
        | SourceKind::Direct
        | SourceKind::Public
        | SourceKind::LocalPublic
        | SourceKind::Bookmarks
        | SourceKind::Favourites => account_source_branches(source, context),
    }
}

fn unscoped_branch(matches: bool) -> Vec<SourceBranch> {
    matches
        .then_some(SourceBranch {
            viewer_account_acct: None,
        })
        .into_iter()
        .collect()
}

fn account_source_branches(
    source: &SourceSpec,
    context: &EvaluationContext<'_>,
) -> Vec<SourceBranch> {
    let candidates = selected_login_accounts(&source.arguments, context);
    let mut seen = HashSet::new();
    let matching = candidates
        .into_iter()
        .filter(|account| account_source_matches(source.kind, context, account))
        .filter(|account| seen.insert(fold(&account.acct)))
        .collect::<Vec<_>>();
    if matches!(source.kind, SourceKind::Bookmarks | SourceKind::Favourites) {
        return matching
            .into_iter()
            .map(|account| SourceBranch {
                viewer_account_acct: Some(account.acct.clone()),
            })
            .collect();
    }
    collapse_implicit_account_scopes(matching)
}

fn collapse_implicit_account_scopes(
    matching: Vec<&super::kq_filter::LoginAccountIdentity>,
) -> Vec<SourceBranch> {
    match matching.as_slice() {
        [] => Vec::new(),
        [account] => vec![SourceBranch {
            viewer_account_acct: Some(account.acct.clone()),
        }],
        _ => vec![SourceBranch {
            viewer_account_acct: None,
        }],
    }
}

fn collapse_membership_scopes(matching: Vec<String>) -> Vec<SourceBranch> {
    let mut seen = HashSet::new();
    let mut matching = matching
        .into_iter()
        .filter(|acct| seen.insert(fold(acct)))
        .collect::<Vec<_>>();
    match matching.len() {
        0 => Vec::new(),
        1 => vec![SourceBranch {
            viewer_account_acct: matching.pop(),
        }],
        _ => vec![SourceBranch {
            viewer_account_acct: None,
        }],
    }
}

fn selected_login_accounts<'a>(
    arguments: &[String],
    context: &'a EvaluationContext<'_>,
) -> Vec<&'a super::kq_filter::LoginAccountIdentity> {
    if arguments.is_empty() {
        return context.login_accounts.iter().collect();
    }
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for selector in arguments {
        for account in context
            .login_accounts
            .iter()
            .filter(|account| login_account_matches_selector(account, selector))
        {
            let key = fold(&login_provider_acct(account));
            if seen.insert(key) {
                selected.push(account);
            }
        }
    }
    selected
}

fn account_source_matches(
    kind: SourceKind,
    context: &EvaluationContext<'_>,
    account: &super::kq_filter::LoginAccountIdentity,
) -> bool {
    let raw_acct = account.acct.as_str();
    match kind {
        SourceKind::Home => membership_matches(context, "home", Some(raw_acct), None),
        SourceKind::Public => membership_matches(context, "public", Some(raw_acct), None),
        SourceKind::LocalPublic => membership_matches(context, "local", Some(raw_acct), None),
        SourceKind::Mentions => mentions_login_account(context, account),
        SourceKind::Direct => {
            let Some(status) = effective_status(context) else {
                return false;
            };
            status.visibility.eq_ignore_ascii_case("direct")
                && (membership_matches(context, "home", Some(raw_acct), None)
                    || membership_matches(context, "direct", Some(raw_acct), None)
                    || author_account(context)
                        .is_some_and(|author| account_matches_login(author, status, account))
                    || mentions_login_account(context, account))
        }
        SourceKind::Bookmarks => {
            viewer_state(context, Some(raw_acct), |state| state.bookmarked) == Truth::True
        }
        SourceKind::Favourites => {
            viewer_state(context, Some(raw_acct), |state| state.favourited) == Truth::True
        }
        _ => false,
    }
}

fn list_source_branches(source: &SourceSpec, context: &EvaluationContext<'_>) -> Vec<SourceBranch> {
    let mut branches = Vec::new();
    for argument in &source.arguments {
        let (account_selector, list_id) = split_scoped_list_id(argument);
        let selectors = account_selector
            .map(|selector| vec![selector.to_string()])
            .unwrap_or_default();
        let mut matching = Vec::new();
        for account in selected_login_accounts(&selectors, context) {
            let raw_acct = account.acct.as_str();
            if membership_matches(context, "list", Some(raw_acct), Some(list_id)) {
                matching.push(raw_acct.to_string());
            }
        }
        branches.extend(collapse_membership_scopes(matching));
    }
    branches
}

fn hashtag_source_branches(tag: &str, context: &EvaluationContext<'_>) -> Vec<SourceBranch> {
    let mut matching = Vec::new();
    for membership in context.memberships.iter().filter(|membership| {
        membership.timeline_type.eq_ignore_ascii_case("hashtag")
            && membership
                .parameter
                .as_deref()
                .is_some_and(|parameter| selector_equal(clean_tag(parameter), tag))
    }) {
        if context
            .login_accounts
            .iter()
            .any(|account| selector_equal(&account.acct, &membership.account_acct))
        {
            matching.push(membership.account_acct.clone());
        }
    }
    collapse_membership_scopes(matching)
}

fn split_scoped_list_id(argument: &str) -> (Option<&str>, &str) {
    let Some((candidate, list_id)) = argument.split_once('/') else {
        return (None, argument);
    };
    // Keep AT URIs and provider list IDs containing slashes opaque. A prefix
    // is a viewer selector only when it looks like an account identity.
    if candidate.contains('@') || candidate.contains('.') || candidate.starts_with('@') {
        (Some(candidate), list_id)
    } else {
        (None, argument)
    }
}

fn text_source_branches(
    source: &SourceSpec,
    context: &EvaluationContext<'_>,
    broad: bool,
) -> Vec<SourceBranch> {
    source
        .arguments
        .iter()
        .filter(|needle| {
            let Some(status) = effective_status(context) else {
                return false;
            };
            if broad {
                search_haystack(status, author_account(context))
                    .is_some_and(|haystack| insensitive_contains(&haystack, needle))
            } else {
                insensitive_contains(&html_to_plain_text(&status.content), needle)
            }
        })
        .map(|_| SourceBranch {
            viewer_account_acct: None,
        })
        .collect()
}

fn search_haystack(status: &DbStatus, account: Option<&DbAccount>) -> Option<String> {
    let mut fields = vec![
        html_to_plain_text(&status.content),
        status.spoiler_text.clone(),
        status.uri.clone(),
        status.url.clone().unwrap_or_default(),
    ];
    if let Some(account) = account {
        fields.push(status_account_acct(account, status));
        fields.push(account.display_name.clone());
    }
    match parse_optional_array::<Tag>(&status.tags_json) {
        ParsedOptionalArray::Values(tags) => {
            fields.extend(tags.into_iter().map(|tag| tag.name));
        }
        // Search remains useful when an unrelated optional JSON projection is
        // damaged. Direct `tags` field access still reports Unknown.
        ParsedOptionalArray::Invalid => {}
    }
    Some(fields.join("\n"))
}

fn conversation_source_matches(context: &EvaluationContext<'_>) -> bool {
    let wrapper_key = status_key(context.wrapper.status);
    if contains_status_key(context.conversation_keys, &wrapper_key) {
        return true;
    }
    context.effective.is_some_and(|effective| {
        contains_status_key(context.conversation_keys, &status_key(effective.status))
    })
}

fn contains_status_key(keys: &[StatusKey], wanted: &StatusKey) -> bool {
    keys.iter().any(|key| {
        key.id == wanted.id
            && key
                .server_domain
                .eq_ignore_ascii_case(&wanted.server_domain)
    })
}

fn status_key(status: &DbStatus) -> StatusKey {
    StatusKey {
        server_domain: status.server_domain.clone(),
        id: status.id.clone(),
    }
}

fn author_matches_selector(context: &EvaluationContext<'_>, selector: &str) -> bool {
    context
        .wrapper
        .account
        .is_some_and(|account| account_matches_selector(account, context.wrapper.status, selector))
}

fn mentions_login_account(
    context: &EvaluationContext<'_>,
    login: &super::kq_filter::LoginAccountIdentity,
) -> bool {
    let Some(status) = effective_status(context) else {
        return false;
    };
    if status
        .in_reply_to_account_id
        .as_deref()
        .is_some_and(|id| identity_equal(id, &login.account_id))
        && status
            .server_domain
            .eq_ignore_ascii_case(&login.server_domain)
    {
        return true;
    }
    match parse_optional_array::<Mention>(&status.mentions_json) {
        ParsedOptionalArray::Values(mentions) => mentions.into_iter().any(|mention| {
            (status
                .server_domain
                .eq_ignore_ascii_case(&login.server_domain)
                && identity_equal(&mention.id, &login.account_id))
                || selector_equal(
                    &status_scoped_acct(&mention.acct, status),
                    &login_provider_acct(login),
                )
        }),
        ParsedOptionalArray::Invalid => false,
    }
}

fn membership_matches(
    context: &EvaluationContext<'_>,
    timeline_type: &str,
    account_acct: Option<&str>,
    parameter: Option<&str>,
) -> bool {
    context.memberships.iter().any(|membership| {
        membership.timeline_type.eq_ignore_ascii_case(timeline_type)
            && account_acct.is_none_or(|acct| selector_equal(&membership.account_acct, acct))
            && parameter.is_none_or(|expected| {
                membership.parameter.as_deref().is_some_and(|actual| {
                    if timeline_type == "hashtag" {
                        selector_equal(clean_tag(actual), clean_tag(expected))
                    } else {
                        actual == expected
                    }
                })
            })
    })
}

fn clean_tag(tag: &str) -> &str {
    tag.trim().trim_start_matches('#')
}

fn login_account_matches_selector(
    account: &super::kq_filter::LoginAccountIdentity,
    selector: &str,
) -> bool {
    let selector = selector.trim();
    let without_marker = selector
        .strip_prefix('@')
        .or_else(|| selector.strip_prefix('#'))
        .unwrap_or(selector);
    identity_equal(without_marker, &account.account_id)
        || selector_equal(without_marker, &login_provider_acct(account))
        || selector_equal(without_marker, &account.acct)
}

fn account_matches_login(
    account: &DbAccount,
    status: &DbStatus,
    login: &super::kq_filter::LoginAccountIdentity,
) -> bool {
    account
        .server_domain
        .eq_ignore_ascii_case(&login.server_domain)
        && identity_equal(&account.id, &login.account_id)
        || selector_equal(
            &status_account_acct(account, status),
            &login_provider_acct(login),
        )
}

fn account_matches_selector(account: &DbAccount, status: &DbStatus, selector: &str) -> bool {
    let selector = selector.trim();
    let without_marker = selector
        .strip_prefix('@')
        .or_else(|| selector.strip_prefix('#'))
        .unwrap_or(selector);
    identity_equal(without_marker, &account.id)
        || selector_equal(without_marker, &status_account_acct(account, status))
        || selector_equal(without_marker, &account.username)
}

fn status_account_acct(account: &DbAccount, status: &DbStatus) -> String {
    status_scoped_acct(&account.acct, status)
}

fn status_scoped_acct(acct: &str, status: &DbStatus) -> String {
    let acct = acct.trim().trim_start_matches('@');
    if status_is_atproto(status) || acct.contains('@') || status.server_domain.is_empty() {
        acct.to_string()
    } else {
        format!("{acct}@{}", status.server_domain)
    }
}

fn account_origin_domain(account: &DbAccount, status: &DbStatus) -> String {
    let acct = account.acct.trim().trim_start_matches('@');
    if let Some((_, domain)) = acct.rsplit_once('@') {
        return domain.to_string();
    }
    if status_is_atproto(status) && acct.contains('.') {
        return acct.to_string();
    }
    account.server_domain.clone()
}

fn status_is_atproto(status: &DbStatus) -> bool {
    status.uri.starts_with("at://") || status.uri.starts_with("repost:")
}

fn login_provider_acct(account: &super::kq_filter::LoginAccountIdentity) -> String {
    let acct = account.acct.trim().trim_start_matches('@');
    if account.server_kind.eq_ignore_ascii_case("bluesky") {
        acct.rsplit_once('@')
            .filter(|(handle, domain)| {
                handle.contains('.') && domain.eq_ignore_ascii_case(&account.server_domain)
            })
            .map(|(handle, _)| handle.to_string())
            .unwrap_or_else(|| acct.to_string())
    } else if acct.contains('@') || account.server_domain.trim().is_empty() {
        acct.to_string()
    } else {
        format!("{acct}@{}", account.server_domain.trim())
    }
}

fn selector_equal(left: &str, right: &str) -> bool {
    fold(left.trim().trim_start_matches('@')) == fold(right.trim().trim_start_matches('@'))
}

fn identity_equal(left: &str, right: &str) -> bool {
    left == right
}

fn insensitive_contains(haystack: &str, needle: &str) -> bool {
    fold(haystack).contains(&fold(needle))
}

fn eval_set(
    items: &[Expr],
    context: &EvaluationContext<'_>,
    branch: &SourceBranch,
    evaluator: &mut Evaluator,
) -> Value {
    let mut values = Vec::new();
    for item in items {
        match evaluator.eval(item, context, branch) {
            Value::Set(nested) => {
                for value in nested {
                    push_unique_scalar(&mut values, value);
                }
            }
            Value::Unknown | Value::Bool(_) => return Value::Unknown,
            scalar => {
                let Some(scalar) = scalar.into_scalar() else {
                    return Value::Unknown;
                };
                push_unique_scalar(&mut values, scalar);
            }
        }
    }
    Value::Set(values)
}

fn eval_unary(operator: UnaryOp, value: Value) -> Value {
    match operator {
        UnaryOp::Not => Value::Bool(value.truth().not()),
        UnaryOp::Negate => match value {
            Value::Number(value) => value
                .checked_neg()
                .map(Value::Number)
                .unwrap_or(Value::Unknown),
            Value::Unknown => Value::Unknown,
            _ => Value::Unknown,
        },
        UnaryOp::Caseful => match value {
            Value::Text { value, .. } => Value::Text {
                value,
                caseful: true,
            },
            Value::Unknown => Value::Unknown,
            _ => Value::Unknown,
        },
    }
}

fn equal_values(left: &Value, right: &Value) -> Truth {
    match (left, right) {
        (Value::Unknown, _) | (_, Value::Unknown) => Truth::Unknown,
        (Value::Bool(left), Value::Bool(right)) => {
            if *left == Truth::Unknown || *right == Truth::Unknown {
                Truth::Unknown
            } else if left == right {
                Truth::True
            } else {
                Truth::False
            }
        }
        (Value::Number(left), Value::Number(right)) => truth(*left == *right),
        (
            Value::Text {
                value: left,
                caseful: left_caseful,
            },
            Value::Text {
                value: right,
                caseful: right_caseful,
            },
        ) => truth(text_equal(left, right, *left_caseful || *right_caseful)),
        (Value::Identity(left), Value::Identity(right)) => truth(identity_equal(left, right)),
        (Value::Identity(left), Value::Text { value: right, .. })
        | (Value::Text { value: right, .. }, Value::Identity(left)) => {
            truth(identity_equal(left, right))
        }
        (Value::Identity(left), Value::Number(right))
        | (Value::Number(right), Value::Identity(left)) => truth(left == &right.to_string()),
        // KQ sets are selected with contains/in. Treating sets as equal would
        // invent semantics that StarryEyes never exposed.
        (Value::Set(_), Value::Set(_)) => Truth::Unknown,
        _ => Truth::Unknown,
    }
}

fn compare_numbers(left: Value, right: Value, compare: impl FnOnce(i64, i64) -> bool) -> Value {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => Value::boolean(compare(left, right)),
        (Value::Unknown, _) | (_, Value::Unknown) => Value::Bool(Truth::Unknown),
        _ => Value::Bool(Truth::Unknown),
    }
}

fn compare_text(left: Value, right: Value, compare: impl FnOnce(&str, &str) -> bool) -> Value {
    let Some((left, left_caseful)) = text_parts(&left) else {
        return Value::Bool(Truth::Unknown);
    };
    let Some((right, right_caseful)) = text_parts(&right) else {
        return Value::Bool(Truth::Unknown);
    };
    if left_caseful || right_caseful {
        Value::boolean(compare(left, right))
    } else {
        Value::boolean(compare(&fold(left), &fold(right)))
    }
}

fn contains(left: Value, right: Value) -> Value {
    if matches!(left, Value::Unknown) || matches!(right, Value::Unknown) {
        return Value::Bool(Truth::Unknown);
    }
    match (left, right) {
        (
            left @ (Value::Text { .. } | Value::Identity(_)),
            right @ (Value::Text { .. } | Value::Identity(_)),
        ) => compare_text(left, right, |haystack, needle| haystack.contains(needle)),
        (Value::Set(values), Value::Set(wanted)) => Value::boolean(
            values
                .iter()
                .any(|candidate| wanted.iter().any(|item| scalar_equal(candidate, item))),
        ),
        (Value::Set(values), scalar) => {
            let Some(wanted) = scalar.into_scalar() else {
                return Value::Bool(Truth::Unknown);
            };
            Value::boolean(
                values
                    .iter()
                    .any(|candidate| scalar_equal(candidate, &wanted)),
            )
        }
        _ => Value::Bool(Truth::Unknown),
    }
}

fn is_in(left: Value, right: Value) -> Value {
    if matches!(left, Value::Unknown) || matches!(right, Value::Unknown) {
        return Value::Bool(Truth::Unknown);
    }
    let Value::Set(values) = right else {
        return Value::Bool(Truth::Unknown);
    };
    match left {
        Value::Set(wanted) => Value::boolean(
            wanted
                .iter()
                .any(|candidate| values.iter().any(|item| scalar_equal(candidate, item))),
        ),
        scalar => {
            let Some(wanted) = scalar.into_scalar() else {
                return Value::Bool(Truth::Unknown);
            };
            Value::boolean(
                values
                    .iter()
                    .any(|candidate| scalar_equal(candidate, &wanted)),
            )
        }
    }
}

fn add(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left
            .checked_add(right)
            .map(Value::Number)
            .unwrap_or(Value::Unknown),
        (
            Value::Text { mut value, caseful },
            Value::Text {
                value: right,
                caseful: right_caseful,
            },
        ) => {
            if value.len().saturating_add(right.len()) > MAX_DERIVED_TEXT_BYTES {
                return Value::Unknown;
            }
            value.push_str(&right);
            Value::Text {
                value,
                caseful: caseful || right_caseful,
            }
        }
        (Value::Set(mut left), Value::Set(right)) => {
            for value in right {
                push_unique_scalar(&mut left, value);
            }
            Value::Set(left)
        }
        (Value::Unknown, _) | (_, Value::Unknown) => Value::Unknown,
        _ => Value::Unknown,
    }
}

fn subtract(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left
            .checked_sub(right)
            .map(Value::Number)
            .unwrap_or(Value::Unknown),
        (Value::Set(left), Value::Set(right)) => Value::Set(
            left.into_iter()
                .filter(|candidate| !right.iter().any(|item| scalar_equal(candidate, item)))
                .collect(),
        ),
        (Value::Unknown, _) | (_, Value::Unknown) => Value::Unknown,
        _ => Value::Unknown,
    }
}

fn multiply(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left
            .checked_mul(right)
            .map(Value::Number)
            .unwrap_or(Value::Unknown),
        (Value::Set(left), Value::Set(right)) => Value::Set(
            left.into_iter()
                .filter(|candidate| right.iter().any(|item| scalar_equal(candidate, item)))
                .collect(),
        ),
        (Value::Unknown, _) | (_, Value::Unknown) => Value::Unknown,
        _ => Value::Unknown,
    }
}

fn divide(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left
            .checked_div(right)
            .map(Value::Number)
            .unwrap_or(Value::Unknown),
        (Value::Unknown, _) | (_, Value::Unknown) => Value::Unknown,
        _ => Value::Unknown,
    }
}

fn text_parts(value: &Value) -> Option<(&str, bool)> {
    match value {
        Value::Text { value, caseful } => Some((value, *caseful)),
        Value::Identity(value) => Some((value, false)),
        _ => None,
    }
}

fn text_equal(left: &str, right: &str, caseful: bool) -> bool {
    if caseful {
        left == right
    } else {
        fold(left) == fold(right)
    }
}

fn scalar_equal(left: &ScalarValue, right: &ScalarValue) -> bool {
    match (left, right) {
        (ScalarValue::Number(left), ScalarValue::Number(right)) => left == right,
        (ScalarValue::Text(left), ScalarValue::Text(right)) => selector_equal(left, right),
        (ScalarValue::Identity(left), ScalarValue::Identity(right)) => identity_equal(left, right),
        (ScalarValue::Identity(left), ScalarValue::Text(right))
        | (ScalarValue::Text(right), ScalarValue::Identity(left)) => identity_equal(left, right),
        (ScalarValue::Identity(left), ScalarValue::Number(right))
        | (ScalarValue::Number(right), ScalarValue::Identity(left)) => left == &right.to_string(),
        (ScalarValue::Text(_), ScalarValue::Number(_))
        | (ScalarValue::Number(_), ScalarValue::Text(_)) => false,
    }
}

fn push_unique_scalar(values: &mut Vec<ScalarValue>, value: ScalarValue) {
    if !values.iter().any(|existing| scalar_equal(existing, &value)) {
        values.push(value);
    }
}

fn truth(value: bool) -> Truth {
    if value {
        Truth::True
    } else {
        Truth::False
    }
}

fn fold(value: &str) -> String {
    let normalizer = ComposingNormalizerBorrowed::new_nfkc();
    let normalized = normalizer.normalize(value);
    let folded = CaseMapper::new().fold_string(&normalized);
    normalizer.normalize(&folded).into_owned()
}

fn field_value(field: Field, context: &EvaluationContext<'_>, branch: &SourceBranch) -> Value {
    match field {
        Field::Boost => Value::boolean(context.wrapper.status.reblog_of_id.is_some()),
        Field::OurAccounts => {
            let mut identities = Vec::new();
            for account in context.login_accounts {
                push_unique_scalar(
                    &mut identities,
                    ScalarValue::Text(login_provider_acct(account)),
                );
                push_unique_scalar(
                    &mut identities,
                    ScalarValue::Identity(account.account_id.clone()),
                );
            }
            Value::Set(identities)
        }
        Field::BoosterId
        | Field::BoosterUsername
        | Field::BoosterAcct
        | Field::BoosterDisplayName
        | Field::BoosterNote
        | Field::BoosterLocked
        | Field::BoosterBot
        | Field::BoosterFollowers
        | Field::BoosterFollowing
        | Field::BoosterStatuses
        | Field::BoosterDomain => booster_field_value(field, context),
        Field::QuoteId | Field::QuoteUrl | Field::QuoteText | Field::QuoteAuthorAcct => {
            quote_field_value(field, context)
        }
        Field::ViewerFavourited
        | Field::ViewerReblogged
        | Field::ViewerMuted
        | Field::ViewerBookmarked
        | Field::ViewerPinned => viewer_field_value(field, context, branch),
        Field::Text
        | Field::RawContent
        | Field::Id
        | Field::Uri
        | Field::Url
        | Field::Application
        | Field::DirectMessage
        | Field::InReplyTo
        | Field::ReplyAccountId
        | Field::IsReply
        | Field::Mentions
        | Field::FavouritesCount
        | Field::ReblogsCount
        | Field::RepliesCount
        | Field::Visibility
        | Field::Language
        | Field::SpoilerText
        | Field::HasSpoiler
        | Field::Sensitive
        | Field::HasMedia
        | Field::MediaCount
        | Field::MediaTypes
        | Field::MediaDescriptions
        | Field::HasImage
        | Field::HasVideo
        | Field::HasAudio
        | Field::HasPoll
        | Field::PollId
        | Field::PollExpired
        | Field::PollMultiple
        | Field::PollVotesCount
        | Field::PollVotersCount
        | Field::PollOptionsCount
        | Field::PollOptions
        | Field::PollExpiresAt
        | Field::HasCard
        | Field::HasQuote
        | Field::Edited
        | Field::EditedAt
        | Field::Domain
        | Field::Hashtags
        | Field::IsPublic
        | Field::IsUnlisted
        | Field::IsPrivate
        | Field::AuthorId
        | Field::AuthorUsername
        | Field::AuthorAcct
        | Field::AuthorDisplayName
        | Field::AuthorNote
        | Field::AuthorLocked
        | Field::AuthorBot
        | Field::AuthorFollowers
        | Field::AuthorFollowing
        | Field::AuthorStatuses
        | Field::AuthorDomain => effective_field_value(field, context),
    }
}

fn effective_field_value(field: Field, context: &EvaluationContext<'_>) -> Value {
    let Some(status) = effective_status(context) else {
        return Value::Unknown;
    };
    match field {
        Field::Text => Value::text(html_to_plain_text(&status.content)),
        Field::RawContent => Value::text(status.content.clone()),
        Field::Id => Value::Identity(status.id.clone()),
        Field::Uri => nonempty_text(&status.uri),
        Field::Url => optional_nonempty_text(status.url.as_deref()),
        Field::Application => status_application(status),
        Field::DirectMessage => Value::boolean(status.visibility.eq_ignore_ascii_case("direct")),
        Field::InReplyTo => optional_identity(status.in_reply_to_id.as_deref()),
        Field::ReplyAccountId => optional_identity(status.in_reply_to_account_id.as_deref()),
        Field::IsReply => Value::boolean(status.in_reply_to_id.is_some()),
        Field::Mentions => mentions_value(status),
        Field::FavouritesCount => Value::Number(status.favourites_count),
        Field::ReblogsCount => Value::Number(status.reblogs_count),
        Field::RepliesCount => Value::Number(status.replies_count),
        Field::Visibility => Value::text(status.visibility.clone()),
        Field::Language => optional_nonempty_text(status.language.as_deref()),
        Field::SpoilerText => Value::text(status.spoiler_text.clone()),
        Field::HasSpoiler => Value::boolean(!status.spoiler_text.is_empty()),
        Field::Sensitive => Value::boolean(status.sensitive),
        Field::HasMedia
        | Field::MediaCount
        | Field::MediaTypes
        | Field::MediaDescriptions
        | Field::HasImage
        | Field::HasVideo
        | Field::HasAudio => media_field_value(field, status),
        Field::HasPoll
        | Field::PollId
        | Field::PollExpired
        | Field::PollMultiple
        | Field::PollVotesCount
        | Field::PollVotersCount
        | Field::PollOptionsCount
        | Field::PollOptions
        | Field::PollExpiresAt => poll_field_value(field, status),
        Field::HasCard => match parse_optional_object::<Card>(&status.card_json) {
            ParsedOptionalObject::Absent => Value::boolean(false),
            ParsedOptionalObject::Value(_) => Value::boolean(true),
            ParsedOptionalObject::Invalid => Value::Bool(Truth::Unknown),
        },
        Field::HasQuote => Value::boolean(status_has_quote(status)),
        Field::Edited => Value::boolean(status.edited_at.is_some()),
        Field::EditedAt => optional_nonempty_text(status.edited_at.as_deref()),
        Field::Domain => Value::text(status.server_domain.clone()),
        Field::Hashtags => tags_value(status),
        Field::IsPublic => Value::boolean(status.visibility.eq_ignore_ascii_case("public")),
        Field::IsUnlisted => Value::boolean(status.visibility.eq_ignore_ascii_case("unlisted")),
        Field::IsPrivate => Value::boolean(status.visibility.eq_ignore_ascii_case("private")),
        Field::AuthorId
        | Field::AuthorUsername
        | Field::AuthorAcct
        | Field::AuthorDisplayName
        | Field::AuthorNote
        | Field::AuthorLocked
        | Field::AuthorBot
        | Field::AuthorFollowers
        | Field::AuthorFollowing
        | Field::AuthorStatuses
        | Field::AuthorDomain => context
            .effective
            .and_then(|view| view.account)
            .map(|account| account_field_value(field, account, status))
            .unwrap_or(Value::Unknown),
        // These variants are routed before this function. Keep this match
        // exhaustive so a newly exposed field cannot silently evaluate false.
        Field::Boost
        | Field::OurAccounts
        | Field::BoosterId
        | Field::BoosterUsername
        | Field::BoosterAcct
        | Field::BoosterDisplayName
        | Field::BoosterNote
        | Field::BoosterLocked
        | Field::BoosterBot
        | Field::BoosterFollowers
        | Field::BoosterFollowing
        | Field::BoosterStatuses
        | Field::BoosterDomain
        | Field::QuoteId
        | Field::QuoteUrl
        | Field::QuoteText
        | Field::QuoteAuthorAcct
        | Field::ViewerFavourited
        | Field::ViewerReblogged
        | Field::ViewerMuted
        | Field::ViewerBookmarked
        | Field::ViewerPinned => Value::Unknown,
    }
}

fn account_field_value(field: Field, account: &DbAccount, status: &DbStatus) -> Value {
    match field {
        Field::AuthorId | Field::BoosterId => Value::Identity(account.id.clone()),
        Field::AuthorUsername | Field::BoosterUsername => Value::text(account.username.clone()),
        Field::AuthorAcct | Field::BoosterAcct => Value::text(status_account_acct(account, status)),
        Field::AuthorDisplayName | Field::BoosterDisplayName => {
            Value::text(account.display_name.clone())
        }
        Field::AuthorNote | Field::BoosterNote => Value::text(html_to_plain_text(&account.note)),
        Field::AuthorLocked | Field::BoosterLocked => Value::boolean(account.locked),
        Field::AuthorBot | Field::BoosterBot => Value::boolean(account.bot),
        Field::AuthorFollowers | Field::BoosterFollowers => Value::Number(account.followers_count),
        Field::AuthorFollowing | Field::BoosterFollowing => Value::Number(account.following_count),
        Field::AuthorStatuses | Field::BoosterStatuses => Value::Number(account.statuses_count),
        Field::AuthorDomain | Field::BoosterDomain => {
            Value::text(account_origin_domain(account, status))
        }
        _ => Value::Unknown,
    }
}

fn booster_field_value(field: Field, context: &EvaluationContext<'_>) -> Value {
    if context.wrapper.status.reblog_of_id.is_none() {
        return Value::Unknown;
    }
    context
        .wrapper
        .account
        .map(|account| account_field_value(field, account, context.wrapper.status))
        .unwrap_or(Value::Unknown)
}

fn quote_field_value(field: Field, context: &EvaluationContext<'_>) -> Value {
    let Some(subject) = effective_status(context) else {
        return Value::Unknown;
    };
    match field {
        Field::QuoteId => context
            .quote
            .map(|quote| Value::Identity(quote.status.id.clone()))
            .unwrap_or(Value::Unknown),
        Field::QuoteUrl => context
            .quote
            .and_then(|quote| {
                quote
                    .status
                    .url
                    .as_deref()
                    .filter(|url| !url.is_empty())
                    .or_else(|| (!quote.status.uri.is_empty()).then_some(quote.status.uri.as_str()))
            })
            .or_else(|| {
                subject
                    .quote_original_url
                    .as_deref()
                    .filter(|url| !url.is_empty())
            })
            .map(|url| Value::text(url.to_string()))
            .unwrap_or(Value::Unknown),
        Field::QuoteText => context
            .quote
            .map(|quote| Value::text(html_to_plain_text(&quote.status.content)))
            .unwrap_or(Value::Unknown),
        Field::QuoteAuthorAcct => context
            .quote
            .and_then(|quote| {
                quote
                    .account
                    .map(|account| Value::text(status_account_acct(account, quote.status)))
            })
            .unwrap_or(Value::Unknown),
        _ => Value::Unknown,
    }
}

fn viewer_field_value(
    field: Field,
    context: &EvaluationContext<'_>,
    branch: &SourceBranch,
) -> Value {
    let selector: fn(&DbStatusViewerState) -> Option<bool> = match field {
        Field::ViewerFavourited => |state: &DbStatusViewerState| state.favourited,
        Field::ViewerReblogged => |state: &DbStatusViewerState| state.reblogged,
        Field::ViewerMuted => |state: &DbStatusViewerState| state.muted,
        Field::ViewerBookmarked => |state: &DbStatusViewerState| state.bookmarked,
        Field::ViewerPinned => |state: &DbStatusViewerState| state.pinned,
        _ => return Value::Unknown,
    };
    Value::Bool(viewer_state(
        context,
        branch.viewer_account_acct.as_deref(),
        selector,
    ))
}

fn viewer_state(
    context: &EvaluationContext<'_>,
    viewer_account_acct: Option<&str>,
    select: impl Fn(&DbStatusViewerState) -> Option<bool>,
) -> Truth {
    let Some(viewer_account_acct) = viewer_account_acct else {
        return Truth::Unknown;
    };
    let Some(subject) = effective_status(context) else {
        return Truth::Unknown;
    };
    let mut matches = context.viewer_states.iter().filter(|state| {
        selector_equal(&state.login_account_acct, viewer_account_acct)
            && state.status_id == subject.id
            && state
                .server_domain
                .eq_ignore_ascii_case(&subject.server_domain)
    });
    let Some(first) = matches.next() else {
        return Truth::Unknown;
    };
    if matches.next().is_some() {
        return Truth::Unknown;
    }
    select(first).map(truth).unwrap_or(Truth::Unknown)
}

fn effective_status<'a>(context: &'a EvaluationContext<'_>) -> Option<&'a DbStatus> {
    context.effective.map(|view| view.status)
}

fn author_account<'a>(context: &'a EvaluationContext<'_>) -> Option<&'a DbAccount> {
    context.effective.and_then(|view| view.account)
}

fn nonempty_text(value: &str) -> Value {
    if value.is_empty() {
        Value::Unknown
    } else {
        Value::text(value.to_string())
    }
}

fn optional_nonempty_text(value: Option<&str>) -> Value {
    value.map(nonempty_text).unwrap_or(Value::Unknown)
}

fn optional_identity(value: Option<&str>) -> Value {
    value
        .filter(|value| !value.is_empty())
        .map(|value| Value::Identity(value.to_string()))
        .unwrap_or(Value::Unknown)
}

fn status_application(status: &DbStatus) -> Value {
    match parse_optional_object::<StatusApplication>(&status.application_json) {
        ParsedOptionalObject::Value(application) => nonempty_text(application.name.trim()),
        ParsedOptionalObject::Absent | ParsedOptionalObject::Invalid => Value::Unknown,
    }
}

fn mentions_value(status: &DbStatus) -> Value {
    let ParsedOptionalArray::Values(mentions) =
        parse_optional_array::<Mention>(&status.mentions_json)
    else {
        return Value::Unknown;
    };
    if status_is_atproto(status) && mentions.is_empty() {
        return Value::Unknown;
    }
    let mut values = Vec::new();
    if let Some(reply_account_id) = status
        .in_reply_to_account_id
        .as_deref()
        .filter(|id| !id.is_empty())
    {
        push_unique_scalar(
            &mut values,
            ScalarValue::Identity(reply_account_id.to_string()),
        );
    }
    for mention in mentions {
        if !mention.id.is_empty() {
            push_unique_scalar(&mut values, ScalarValue::Identity(mention.id));
        }
        if !mention.acct.is_empty() {
            push_unique_scalar(
                &mut values,
                ScalarValue::Text(status_scoped_acct(&mention.acct, status)),
            );
        }
        if !mention.username.is_empty() {
            push_unique_scalar(&mut values, ScalarValue::Text(mention.username));
        }
    }
    Value::Set(values)
}

fn tags_value(status: &DbStatus) -> Value {
    match parse_optional_array::<Tag>(&status.tags_json) {
        ParsedOptionalArray::Values(tags) if status_is_atproto(status) && tags.is_empty() => {
            Value::Unknown
        }
        ParsedOptionalArray::Values(tags) => {
            let mut values = Vec::new();
            for tag in tags.into_iter().filter(|tag| !tag.name.is_empty()) {
                push_unique_scalar(&mut values, ScalarValue::Text(tag.name));
            }
            Value::Set(values)
        }
        ParsedOptionalArray::Invalid => Value::Unknown,
    }
}

fn media_field_value(field: Field, status: &DbStatus) -> Value {
    let ParsedOptionalArray::Values(media) =
        parse_optional_array::<MediaAttachment>(&status.media_attachments_json)
    else {
        return Value::Unknown;
    };
    match field {
        Field::HasMedia => Value::boolean(!media.is_empty()),
        Field::MediaCount => usize_number(media.len()),
        Field::MediaTypes => {
            let mut values = Vec::new();
            for attachment in &media {
                push_unique_scalar(
                    &mut values,
                    ScalarValue::Text(normalize_media_type(&attachment.media_type).to_string()),
                );
            }
            Value::Set(values)
        }
        Field::MediaDescriptions => {
            let mut values = Vec::new();
            for description in media
                .iter()
                .filter_map(|attachment| attachment.description.as_deref())
                .filter(|description| !description.is_empty())
            {
                push_unique_scalar(&mut values, ScalarValue::Text(description.to_string()));
            }
            Value::Set(values)
        }
        Field::HasImage => Value::boolean(
            media
                .iter()
                .any(|attachment| normalize_media_type(&attachment.media_type) == "image"),
        ),
        Field::HasVideo => Value::boolean(media.iter().any(|attachment| {
            matches!(
                normalize_media_type(&attachment.media_type),
                "video" | "gifv"
            )
        })),
        Field::HasAudio => Value::boolean(
            media
                .iter()
                .any(|attachment| normalize_media_type(&attachment.media_type) == "audio"),
        ),
        _ => Value::Unknown,
    }
}

fn normalize_media_type(media_type: &str) -> &str {
    match media_type {
        "image" => "image",
        "gifv" => "gifv",
        "video" => "video",
        "audio" => "audio",
        _ => "unknown",
    }
}

fn poll_field_value(field: Field, status: &DbStatus) -> Value {
    match parse_optional_object::<Poll>(&status.poll_json) {
        ParsedOptionalObject::Absent => {
            if field == Field::HasPoll {
                Value::boolean(false)
            } else {
                Value::Unknown
            }
        }
        ParsedOptionalObject::Invalid => Value::Unknown,
        ParsedOptionalObject::Value(poll) => match field {
            Field::HasPoll => Value::boolean(true),
            Field::PollId => optional_identity(Some(&poll.id)),
            Field::PollExpired => Value::boolean(poll.expired),
            Field::PollMultiple => Value::boolean(poll.multiple),
            Field::PollVotesCount => Value::Number(poll.votes_count),
            Field::PollVotersCount => poll
                .voters_count
                .map(Value::Number)
                .unwrap_or(Value::Unknown),
            Field::PollOptionsCount => usize_number(poll.options.len()),
            Field::PollOptions => {
                let mut values = Vec::new();
                for option in poll
                    .options
                    .into_iter()
                    .filter(|option| !option.title.is_empty())
                {
                    push_unique_scalar(&mut values, ScalarValue::Text(option.title));
                }
                Value::Set(values)
            }
            Field::PollExpiresAt => poll
                .expires_at
                .map(|expires_at| Value::text(expires_at.to_rfc3339()))
                .unwrap_or(Value::Unknown),
            _ => Value::Unknown,
        },
    }
}

fn usize_number(value: usize) -> Value {
    i64::try_from(value)
        .map(Value::Number)
        .unwrap_or(Value::Unknown)
}

fn status_has_quote(status: &DbStatus) -> bool {
    status.quote_id.as_deref().is_some_and(|id| !id.is_empty())
        || status
            .quote_original_url
            .as_deref()
            .is_some_and(|url| !url.is_empty())
}

enum ParsedOptionalArray<T> {
    Values(Vec<T>),
    Invalid,
}

fn parse_optional_array<T: serde::de::DeserializeOwned>(
    json: &Option<String>,
) -> ParsedOptionalArray<T> {
    match json.as_deref() {
        None => ParsedOptionalArray::Values(Vec::new()),
        Some(json) if json.len() > MAX_JSON_BYTES => ParsedOptionalArray::Invalid,
        Some(json) => serde_json::from_str(json)
            .map(ParsedOptionalArray::Values)
            .unwrap_or(ParsedOptionalArray::Invalid),
    }
}

enum ParsedOptionalObject<T> {
    Absent,
    Value(T),
    Invalid,
}

fn parse_optional_object<T: serde::de::DeserializeOwned>(
    json: &Option<String>,
) -> ParsedOptionalObject<T> {
    match json.as_deref() {
        None => ParsedOptionalObject::Absent,
        Some(json) if json.len() > MAX_JSON_BYTES => ParsedOptionalObject::Invalid,
        Some(json) => serde_json::from_str(json)
            .map(ParsedOptionalObject::Value)
            .unwrap_or(ParsedOptionalObject::Invalid),
    }
}

/// Convert provider HTML to stable plain text without accepting markup as KQ
/// syntax. Mastodon content is normally sanitized, but malformed cached input
/// is handled without panicking. This mirrors the frontend display contract:
/// BR and closing block tags are newlines and nested entities are decoded.
pub(crate) fn html_to_plain_text(html: &str) -> String {
    let mut output = String::with_capacity(html.len().min(MAX_DERIVED_TEXT_BYTES));
    let mut chars = html.chars().peekable();
    let mut suppressed_tag: Option<String> = None;
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
                if tag.len() < 256 {
                    tag.push(next);
                }
            }
            let (closing, name) = html_tag_name(&tag);
            if suppressed_tag
                .as_deref()
                .is_some_and(|suppressed| closing && suppressed == name)
            {
                suppressed_tag = None;
                continue;
            }
            if suppressed_tag.is_some() {
                continue;
            }
            if !closing && matches!(name.as_str(), "script" | "style") {
                suppressed_tag = Some(name);
            } else if name == "br" {
                output.push('\n');
            } else if closing && is_block_tag(&name) {
                push_newline(&mut output);
            }
            continue;
        }
        if suppressed_tag.is_none() {
            output.push(ch);
        }
        if output.len() >= MAX_DERIVED_TEXT_BYTES {
            break;
        }
    }
    decode_nested_html_entities(output.trim())
        .trim()
        .to_string()
}

fn html_tag_name(raw_tag: &str) -> (bool, String) {
    let trimmed = raw_tag.trim_start();
    let closing = trimmed.starts_with('/');
    let name = trimmed
        .trim_start_matches('/')
        .split(|ch: char| ch.is_whitespace() || ch == '/')
        .next()
        .unwrap_or_default();
    (closing, name.to_ascii_lowercase())
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "footer"
            | "header"
            | "li"
            | "main"
            | "nav"
            | "ul"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

fn push_newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn decode_nested_html_entities(value: &str) -> String {
    let mut decoded = value.to_string();
    for _ in 0..8 {
        let next = decode_html_entities_once(&decoded);
        if next == decoded {
            break;
        }
        decoded = next;
    }
    decoded
}

fn decode_html_entities_once(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            output.push(ch);
            continue;
        }
        let mut entity = String::new();
        let mut terminated = false;
        while entity.len() <= 32 {
            let Some(next) = chars.peek().copied() else {
                break;
            };
            if next == ';' {
                chars.next();
                terminated = true;
                break;
            }
            if !(next.is_ascii_alphanumeric() || matches!(next, '#' | 'x' | 'X')) {
                break;
            }
            chars.next();
            entity.push(next);
        }
        if terminated {
            if let Some(decoded) = decode_html_entity(&entity) {
                output.push(decoded);
            } else {
                output.push('&');
                output.push_str(&entity);
                output.push(';');
            }
        } else {
            output.push('&');
            output.push_str(&entity);
        }
    }
    output
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "nbsp" => Some(' '),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "hellip" => Some('…'),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "trade" => Some('™'),
        "eacute" => Some('é'),
        value if value.starts_with("#x") || value.starts_with("#X") => {
            u32::from_str_radix(&value[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        value if value.starts_with('#') => value[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kq_filter::{
        compile_query, EvaluationContext, Evaluator as QueryEvaluator, LoginAccountIdentity,
        StatusView, TimelineMembership,
    };

    fn account(id: &str, username: &str, acct: &str, domain: &str) -> DbAccount {
        DbAccount {
            id: id.to_string(),
            server_domain: domain.to_string(),
            username: username.to_string(),
            acct: acct.to_string(),
            display_name: format!("{username} display"),
            note: "<p>profile &amp; note</p>".to_string(),
            avatar: String::new(),
            avatar_static: String::new(),
            header: String::new(),
            locked: false,
            bot: false,
            followers_count: 10,
            following_count: 20,
            statuses_count: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            fetched_at: "2025-01-01T00:00:00Z".to_string(),
            fields_json: None,
            emojis_json: None,
        }
    }

    fn status(id: &str, domain: &str, account_id: &str) -> DbStatus {
        DbStatus {
            id: id.to_string(),
            server_domain: domain.to_string(),
            uri: format!("https://{domain}/statuses/{id}"),
            url: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            edited_at: None,
            account_id: account_id.to_string(),
            content: "<p>snow &amp; ice</p>".to_string(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            reblogs_count: 2,
            favourites_count: 3,
            replies_count: 1,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog_of_id: None,
            language: Some("en".to_string()),
            pinned: None,
            favourited: None,
            reblogged: None,
            muted: None,
            bookmarked: None,
            poll_json: None,
            card_json: None,
            application_json: None,
            mentions_json: None,
            tags_json: None,
            emojis_json: None,
            media_attachments_json: None,
            fetched_at: "2025-01-01T00:00:00Z".to_string(),
            quote_id: None,
            quote_original_url: None,
        }
    }

    fn login(
        acct: &str,
        domain: &str,
        account_id: &str,
        server_kind: &str,
    ) -> LoginAccountIdentity {
        LoginAccountIdentity {
            acct: acct.to_string(),
            server_domain: domain.to_string(),
            account_id: account_id.to_string(),
            display_name: acct.to_string(),
            server_kind: server_kind.to_string(),
            is_active: true,
        }
    }

    fn viewer(acct: &str, status: &DbStatus) -> DbStatusViewerState {
        DbStatusViewerState {
            login_account_acct: acct.to_string(),
            status_id: status.id.clone(),
            server_domain: status.server_domain.clone(),
            favourited: None,
            reblogged: None,
            muted: None,
            bookmarked: None,
            pinned: None,
            updated_at: "2025-01-01T00:00:00Z".to_string(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn query_matches(
        query: &str,
        wrapper_status: &DbStatus,
        wrapper_account: Option<&DbAccount>,
        effective: Option<(&DbStatus, Option<&DbAccount>)>,
        quote: Option<(&DbStatus, Option<&DbAccount>)>,
        logins: &[LoginAccountIdentity],
        memberships: &[TimelineMembership],
        viewer_states: &[DbStatusViewerState],
        conversations: &[StatusKey],
        _viewer_hint: Option<&str>,
    ) -> bool {
        let wrapper = StatusView::new(wrapper_status, wrapper_account);
        let effective = effective.map(|(status, account)| StatusView::new(status, account));
        let quote = quote.map(|(status, account)| StatusView::new(status, account));
        let mut context = EvaluationContext::new(wrapper, effective);
        context.quote = quote;
        context.login_accounts = logins;
        context.memberships = memberships;
        context.viewer_states = viewer_states;
        context.conversation_keys = conversations;
        let compiled = compile_query(query).unwrap_or_else(|error| panic!("{query}: {error}"));
        QueryEvaluator::new().matches(&compiled, &context)
    }

    fn local_matches(query: &str, status: &DbStatus, account: &DbAccount) -> bool {
        query_matches(
            query,
            status,
            Some(account),
            Some((status, Some(account))),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        )
    }

    #[test]
    fn plain_text_matches_display_line_break_and_nested_entity_contract() {
        assert_eq!(
            html_to_plain_text("<p>first<br>second</p><p>&amp;#34;third&amp;#34;</p>"),
            "first\nsecond\n\"third\""
        );
        assert_eq!(
            html_to_plain_text("safe<SCRIPT>not visible</SCRIPT><style>x</style> text"),
            "safe text"
        );
        assert_eq!(
            html_to_plain_text("fish&nbsp;&amp;&nbsp;chips"),
            "fish & chips"
        );
        assert_eq!(html_to_plain_text("a<br><br>b"), "a\n\nb");
        assert_eq!(
            html_to_plain_text("&ldquo;caf&eacute;&rdquo;&nbsp;&copy;"),
            "“café” ©"
        );
    }

    #[test]
    fn kleene_unknown_short_circuits_and_arithmetic_is_checked_and_right_associative() {
        let author = account("author", "alice", "alice", "example.test");
        let row = status("row", "example.test", "author");
        assert!(!local_matches("where !(url == \"missing\")", &row, &author));
        assert!(local_matches(
            "where url == \"missing\" | !boost",
            &row,
            &author
        ));
        assert!(local_matches("where 10 - 3 - 2 == 9", &row, &author));
        assert!(!local_matches("where 4 / 0 == 0", &row, &author));
        assert!(!local_matches(
            "where 9223372036854775807 + 1 == 0",
            &row,
            &author
        ));
        assert!(!local_matches(
            "where -9223372036854775807 - 1 / -1 == 0",
            &row,
            &author
        ));
        assert_eq!(Truth::Unknown.and(Truth::False), Truth::False);
        assert_eq!(Truth::Unknown.or(Truth::True), Truth::True);
        assert_eq!(Truth::Unknown.not(), Truth::Unknown);
        assert!(matches!(
            divide(Value::Number(i64::MIN), Value::Number(-1)),
            Value::Unknown
        ));
    }

    #[test]
    fn opaque_ids_are_exact_while_accts_are_unicode_folded_and_our_has_both() {
        let author = account("AbC", "alice", "alice@origin.test", "cache.test");
        let mut row = status("AbC", "cache.test", "AbC");
        row.uri = "https://origin.test/@alice/AbC".to_string();
        let logins = [login("alice@origin.test", "cache.test", "AbC", "mastodon")];

        assert!(local_matches("where id == #AbC", &row, &author));
        assert!(!local_matches("where id == #abc", &row, &author));
        assert!(local_matches("where id == \"AbC\"", &row, &author));
        assert!(!local_matches("where id == \"abc\"", &row, &author));
        assert!(query_matches(
            "where user in our & #AbC in our & author.acct == \"ALICE@ORIGIN.TEST\"",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &[],
            &[],
            &[],
            None,
        ));
        assert!(local_matches(
            "where author.domain == \"origin.test\"",
            &row,
            &author
        ));

        let numeric = status("123", "cache.test", "AbC");
        assert!(local_matches("where id == 123", &numeric, &author));
    }

    #[test]
    fn provider_account_canonicalization_keeps_raw_database_scope() {
        let author = account(
            "did:plc:alice",
            "alice.bsky.social",
            "alice.bsky.social",
            "bsky.social",
        );
        let mut row = status("at-id", "bsky.social", "did:plc:alice");
        row.uri = "at://did:plc:alice/app.bsky.feed.post/at-id".to_string();
        let bluesky_login = login(
            "alice.bsky.social@bsky.social",
            "bsky.social",
            "did:plc:alice",
            "bluesky",
        );
        let memberships = [TimelineMembership::new(
            "home",
            "alice.bsky.social@bsky.social",
            None,
        )];
        assert_eq!(login_provider_acct(&bluesky_login), "alice.bsky.social");
        assert_eq!(
            login_provider_acct(&login(
                "john.doe",
                "example.test",
                "mastodon-id",
                "mastodon",
            )),
            "john.doe@example.test"
        );
        assert_eq!(
            login_provider_acct(&login(
                "alice@remote.test",
                "cache.test",
                "misskey-id",
                "misskey",
            )),
            "alice@remote.test"
        );
        assert!(query_matches(
            "from home:\"alice.bsky.social\" where author.acct == \"ALICE.BSKY.SOCIAL\" & author.domain == \"alice.bsky.social\"",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &[bluesky_login],
            &memberships,
            &[],
            &[],
            None,
        ));
    }

    #[test]
    fn boost_source_uses_wrapper_but_predicate_fields_use_effective_original() {
        let booster = account("boost-id", "booster", "booster", "example.test");
        let original_author = account("author-id", "author", "author", "remote.test");
        let mut wrapper = status("wrapper", "example.test", "boost-id");
        wrapper.reblog_of_id = Some("original".to_string());
        wrapper.content.clear();
        let mut original = status("original", "remote.test", "author-id");
        original.content = "<p>original body</p>".to_string();

        assert!(query_matches(
            "from user:\"booster@example.test\" where user.acct == \"author@remote.test\" & booster.acct == \"booster@example.test\"",
            &wrapper,
            Some(&booster),
            Some((&original, Some(&original_author))),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        ));
        assert!(!query_matches(
            "from user:\"author@remote.test\"",
            &wrapper,
            Some(&booster),
            Some((&original, Some(&original_author))),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        ));
        assert!(query_matches(
            "from local where boost & booster.acct == \"booster@example.test\"",
            &wrapper,
            Some(&booster),
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        ));
        assert!(!query_matches(
            "from local where text contains \"wrapper\"",
            &wrapper,
            Some(&booster),
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        ));
    }

    #[test]
    fn bluesky_synthetic_repost_keeps_booster_handle_and_domain() {
        let reposter = account(
            "did:plc:reposter",
            "reposter.handle",
            "reposter.handle",
            "bsky.social",
        );
        let original_author = account(
            "did:plc:author",
            "author.handle",
            "author.handle",
            "bsky.social",
        );
        let mut wrapper = status(
            "repost:did:plc:reposter:at://did:plc:author/app.bsky.feed.post/one",
            "bsky.social",
            "did:plc:reposter",
        );
        wrapper.uri = wrapper.id.clone();
        wrapper.reblog_of_id = Some("at://did:plc:author/app.bsky.feed.post/one".to_string());
        let mut original = status(
            "at://did:plc:author/app.bsky.feed.post/one",
            "bsky.social",
            "did:plc:author",
        );
        original.uri = original.id.clone();

        assert!(query_matches(
            "from user:\"reposter.handle\" where booster.acct == \"reposter.handle\" & booster.domain == \"reposter.handle\"",
            &wrapper,
            Some(&reposter),
            Some((&original, Some(&original_author))),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
        ));
    }

    #[test]
    fn viewer_scope_is_explicit_and_never_aggregates_ambiguous_accounts() {
        let author = account("author", "author", "author", "example.test");
        let row = status("row", "example.test", "author");
        let logins = [
            login("alice@example.test", "example.test", "shared", "mastodon"),
            login("bob@example.test", "example.test", "shared", "mastodon"),
        ];
        let memberships = [
            TimelineMembership::new("home", "alice@example.test", None),
            TimelineMembership::new("home", "bob@example.test", None),
        ];
        let mut alice = viewer("alice@example.test", &row);
        alice.favourited = Some(false);
        alice.bookmarked = Some(false);
        let mut bob = viewer("bob@example.test", &row);
        bob.favourited = Some(true);
        bob.bookmarked = Some(true);
        let states = [alice, bob];

        assert!(!query_matches(
            "from home where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &[],
            None,
        ));
        assert!(query_matches(
            "from home:\"bob@example.test\" where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &[],
            None,
        ));
        assert!(!query_matches(
            "from home:\"#shared\" where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &[],
            None,
        ));
        assert!(!query_matches(
            "from local where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &[],
            Some("bob@example.test"),
        ));
        assert!(query_matches(
            "from bookmarks where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &[],
            None,
        ));
    }

    #[test]
    fn mention_account_ids_are_scoped_to_the_receiving_server() {
        let author = account("author", "author", "author", "example.test");
        let mut row = status("row", "example.test", "author");
        row.mentions_json = Some(
            r#"[{"id":"shared","username":"alice","acct":"alice@example.test","url":"https://example.test/@alice"}]"#
                .to_string(),
        );
        let logins = [
            login("alice@example.test", "example.test", "shared", "mastodon"),
            login("other@other.test", "other.test", "shared", "mastodon"),
        ];
        let mut alice = viewer("alice@example.test", &row);
        alice.favourited = Some(true);
        let states = [alice];
        assert!(query_matches(
            "from mentions where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &[],
            &states,
            &[],
            None,
        ));
    }

    #[test]
    fn list_hashtag_search_track_and_conversation_sources_stay_cache_local() {
        let author = account("author", "author", "author", "example.test");
        let mut row = status("row", "example.test", "author");
        row.tags_json = Some("not-json".to_string());
        let logins = [
            login("alice@example.test", "example.test", "alice", "mastodon"),
            login("bob@example.test", "example.test", "bob", "mastodon"),
        ];
        let memberships = [
            TimelineMembership::new("list", "alice@example.test", Some("friends".to_string())),
            TimelineMembership::new("list", "bob@example.test", Some("friends".to_string())),
            TimelineMembership::new("hashtag", "alice@example.test", Some("rust".to_string())),
            TimelineMembership::new("hashtag", "bob@example.test", Some("rust".to_string())),
        ];
        let mut bob = viewer("bob@example.test", &row);
        bob.favourited = Some(true);
        let states = [bob];
        let conversations = [StatusKey::new("example.test", "row")];

        assert!(!query_matches(
            "from list:\"friends\" where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &conversations,
            None,
        ));
        assert!(query_matches(
            "from list:\"bob@example.test/friends\" where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &conversations,
            None,
        ));
        assert!(!query_matches(
            "from hashtag:\"rust\" where viewer.favourited",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            None,
            &logins,
            &memberships,
            &states,
            &conversations,
            None,
        ));
        for query in [
            "from search:\"SNOW\"",
            "from track:\"snow\"",
            "from conversation:\"example.test/row\"",
        ] {
            assert!(query_matches(
                query,
                &row,
                Some(&author),
                Some((&row, Some(&author))),
                None,
                &logins,
                &memberships,
                &states,
                &conversations,
                None,
            ));
        }
    }

    #[test]
    fn absent_corrupt_and_provider_dropped_json_remain_distinct() {
        let author = account("author", "author", "author", "example.test");
        let mut row = status("row", "example.test", "author");
        assert!(local_matches(
            "where !has_media & !has_poll & !has_card",
            &row,
            &author
        ));

        row.media_attachments_json = Some("{".to_string());
        row.poll_json = Some("{".to_string());
        row.card_json = Some("{".to_string());
        assert!(!local_matches(
            "where !has_media | !has_poll | !has_card",
            &row,
            &author
        ));

        row.media_attachments_json = Some(
            r#"[{"id":"m1","type":"image","description":"alt text"},{"id":"m2","type":"gifv"}]"#
                .to_string(),
        );
        row.poll_json = Some(
            r#"{"id":"poll","expired":false,"multiple":true,"votes_count":5,"voters_count":4,"options":[{"title":"yes"},{"title":"no"}]}"#
                .to_string(),
        );
        row.card_json = Some(r#"{"url":"https://card.test/"}"#.to_string());
        assert!(local_matches(
            "where has_image & has_video & media.descriptions contains \"ALT TEXT\" & poll.votes_count == 5 & poll.options contains \"yes\" & has_card",
            &row,
            &author,
        ));

        row.tags_json = Some(format!("[\"{}\"]", "x".repeat(MAX_JSON_BYTES + 1)));
        let view = StatusView::new(&row, Some(&author));
        let context = EvaluationContext::new(view, Some(view));
        let branch = SourceBranch {
            viewer_account_acct: None,
        };
        assert!(matches!(
            field_value(Field::Hashtags, &context, &branch),
            Value::Unknown
        ));

        let mut bluesky = status("at-id", "bsky.social", "did:plc:alice");
        bluesky.uri = "at://did:plc:alice/app.bsky.feed.post/at-id".to_string();
        bluesky.mentions_json = Some("[]".to_string());
        bluesky.tags_json = Some("[]".to_string());
        let bluesky_view = StatusView::new(&bluesky, Some(&author));
        let bluesky_context = EvaluationContext::new(bluesky_view, Some(bluesky_view));
        assert!(matches!(
            field_value(Field::Mentions, &bluesky_context, &branch),
            Value::Unknown
        ));
        assert!(matches!(
            field_value(Field::Hashtags, &bluesky_context, &branch),
            Value::Unknown
        ));
    }

    #[test]
    fn quote_envelope_is_known_but_target_fields_require_hydration() {
        let author = account("author", "author", "author", "example.test");
        let quote_author = account("quoted", "quoted", "quoted", "remote.test");
        let mut row = status("row", "example.test", "author");
        row.quote_id = Some("quote-id".to_string());
        row.quote_original_url = Some("https://remote.test/@quoted/quote-id".to_string());
        assert!(local_matches("where has_quote", &row, &author));
        assert!(!local_matches("where quote.id == #quote-id", &row, &author));
        assert!(local_matches(
            "where quote.url == \"https://remote.test/@quoted/quote-id\"",
            &row,
            &author
        ));

        let mut quote = status("quote-id", "remote.test", "quoted");
        quote.content = "<p>quoted<br>body</p>".to_string();
        assert!(query_matches(
            "where quote.id == #quote-id & quote.text == \"quoted\nbody\" & quote.author.acct == \"quoted@remote.test\"",
            &row,
            Some(&author),
            Some((&row, Some(&author))),
            Some((&quote, Some(&quote_author))),
            &[],
            &[],
            &[],
            &[],
            None,
        ));
    }

    #[test]
    fn regex_is_case_sensitive_and_bounded_and_raw_content_stays_raw() {
        let author = account("author", "author", "author", "example.test");
        let mut row = status("row", "example.test", "author");
        assert!(local_matches("where text regex \"snow\"", &row, &author));
        assert!(!local_matches("where text regex \"SNOW\"", &row, &author));
        assert!(local_matches(
            "where raw_content contains \"<p>\" & author.note == \"profile & note\"",
            &row,
            &author
        ));
        row.content = "a".repeat(MAX_REGEX_INPUT_BYTES + 1);
        assert!(!local_matches(
            "where !(raw_content regex \"a\")",
            &row,
            &author
        ));
    }
}
