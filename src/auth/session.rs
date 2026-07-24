use std::collections::HashMap;

use crate::api::client::ApiClient;
use crate::mastodon::types::account::Account;

#[derive(Clone)]
pub struct AccountSession {
    pub acct: String,
    pub domain: String,
    pub client: ApiClient,
    pub account_info: Account,
}

pub struct SessionManager {
    sessions: HashMap<String, AccountSession>,
    active_acct: Option<String>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            active_acct: None,
        }
    }

    pub fn add_session(&mut self, session: AccountSession) {
        let acct = session.acct.clone();
        if self.active_acct.is_none() {
            self.active_acct = Some(acct.clone());
        }
        self.sessions.insert(acct, session);
    }

    pub fn active_session(&self) -> Option<&AccountSession> {
        self.active_acct
            .as_ref()
            .and_then(|acct| self.sessions.get(acct))
    }

    pub fn set_active(&mut self, acct: &str) -> bool {
        if self.sessions.contains_key(acct) {
            self.active_acct = Some(acct.to_string());
            true
        } else {
            false
        }
    }

    pub fn remove_session(&mut self, acct: &str) {
        self.sessions.remove(acct);
        if self.active_acct.as_deref() == Some(acct) {
            self.active_acct = self.sessions.keys().next().cloned();
        }
    }

    pub fn sessions(&self) -> &HashMap<String, AccountSession> {
        &self.sessions
    }
}
