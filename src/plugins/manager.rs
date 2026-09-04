use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Once};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boa_engine::builtins::promise::PromiseState;
use boa_engine::error::EngineError;
use boa_engine::module::SimpleModuleLoader;
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::object::JsObject;
use boa_engine::{js_string, Context, JsError, JsString, JsValue, Module, Source};
use boa_runtime::extensions::{ConsoleExtension, FetchExtension};
use boa_runtime::{ConsoleState, Logger};
use serde_json::Value;

use crate::plugin_runtime_limits::{
    PLUGIN_CALLBACK_DEADLINE_MS, PLUGIN_CALLBACK_REPLY_TIMEOUT_MS, PLUGIN_QUICK_REPLY_TIMEOUT_MS,
};

use super::executor::{JobControlError, TurnJobExecutor};
use super::fetcher::BoundedReqwestFetcher;
use super::{
    ComposeButtonDescriptor, PluginHook, PluginHookToken, PluginInfo, PluginLogEntry,
    PluginLogLevel, PluginSnapshot, PluginState,
};

const ACTOR_TICK: Duration = Duration::from_millis(10);
const PROMISE_DEADLINE: Duration = Duration::from_millis(PLUGIN_CALLBACK_DEADLINE_MS as u64);
const QUICK_REPLY_DEADLINE: Duration = Duration::from_millis(PLUGIN_QUICK_REPLY_TIMEOUT_MS as u64);
const CALLBACK_REPLY_DEADLINE: Duration =
    Duration::from_millis(PLUGIN_CALLBACK_REPLY_TIMEOUT_MS as u64);
const MAX_JOBS_PER_CALLBACK: u64 = 8_192;
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
const COMMAND_QUEUE_CAPACITY: usize = 64;
const LOOP_ITERATION_LIMIT: u64 = 1_000_000;
const MAX_LOG_ENTRIES: usize = 500;
const SUPPORTED_PLUGIN_VERSION: u32 = 1;

type SharedLogs = Arc<Mutex<VecDeque<PluginLogEntry>>>;
type SnapshotReply = SyncSender<Result<PluginSnapshot, String>>;
type ValueReply = SyncSender<Result<Value, String>>;
type HookTokenReply = SyncSender<Result<Option<PluginHookToken>, String>>;

const PROTECTED_HOOK_METADATA: [&str; 4] = [
    "_awayukiAction",
    "_awayukiActingAccountAcct",
    "actingAccountAcct",
    "operationId",
];

#[derive(Debug)]
struct PluginExecutionError {
    message: String,
    poison_runtime: bool,
}

impl PluginExecutionError {
    fn ordinary(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            poison_runtime: false,
        }
    }

    fn control_limit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            poison_runtime: true,
        }
    }
}

impl fmt::Display for PluginExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone)]
pub struct PluginManager {
    inner: Arc<PluginManagerInner>,
}

struct PluginManagerInner {
    directory: PathBuf,
    sender: SyncSender<QueuedCommand>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for PluginManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginManager")
            .field("directory", &self.inner.directory)
            .finish_non_exhaustive()
    }
}

impl PluginManager {
    /// Starts the plugin actor and returns immediately.
    ///
    /// Directory scanning and JavaScript module evaluation happen on the
    /// dedicated worker thread before it handles the first command.
    pub fn start(directory: PathBuf) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let worker_directory = directory.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("awayuki-plugin-runtime".to_owned())
            .spawn(move || PluginActor::run(worker_directory, receiver, worker_stop))
            .map_err(|error| format!("failed to start plugin runtime: {error}"))?;

        Ok(Self {
            inner: Arc::new(PluginManagerInner {
                directory,
                sender,
                stop,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    pub fn directory(&self) -> &Path {
        &self.inner.directory
    }

    pub fn snapshot(&self) -> Result<PluginSnapshot, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Command::Snapshot(reply),
            QUICK_REPLY_DEADLINE,
            Duration::ZERO,
        )?;
        receive_result(receiver, QUICK_REPLY_DEADLINE, "snapshot")
    }

    pub fn reload_all(&self) -> Result<PluginSnapshot, String> {
        self.request_snapshot(
            Command::ReloadAll,
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
            "reload all",
        )
    }

    pub fn reload_plugin(&self, plugin_id: &str) -> Result<PluginSnapshot, String> {
        let plugin_id = plugin_id.to_owned();
        self.request_snapshot(
            move |reply| Command::ReloadPlugin { plugin_id, reply },
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
            "reload plugin",
        )
    }

    pub fn unload_plugin(&self, plugin_id: &str) -> Result<PluginSnapshot, String> {
        let plugin_id = plugin_id.to_owned();
        self.request_snapshot(
            move |reply| Command::UnloadPlugin { plugin_id, reply },
            QUICK_REPLY_DEADLINE,
            Duration::ZERO,
            "unload plugin",
        )
    }

    pub fn has_hook(&self, hook: PluginHook) -> Result<bool, String> {
        self.has_hook_with_deadline(hook, QUICK_REPLY_DEADLINE)
    }

    /// Returns a token for the current hook set, or `None` when no plugin has
    /// registered the hook. Before-hook callers that perform asynchronous work
    /// between this check and invocation must use the token with
    /// [`Self::run_hook_checked`] so reload/unload cannot silently bypass a hook.
    pub fn hook_token(&self, hook: PluginHook) -> Result<Option<PluginHookToken>, String> {
        self.hook_token_with_deadline(hook, QUICK_REPLY_DEADLINE)
    }

    pub(super) fn has_hook_with_deadline(
        &self,
        hook: PluginHook,
        deadline: Duration,
    ) -> Result<bool, String> {
        self.hook_token_with_deadline(hook, deadline)
            .map(|token| token.is_some())
    }

    fn hook_token_with_deadline(
        &self,
        hook: PluginHook,
        deadline: Duration,
    ) -> Result<Option<PluginHookToken>, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Command::HookToken { hook, reply }, deadline, Duration::ZERO)?;
        match receiver.recv_timeout(deadline) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "plugin runtime timed out after {}ms while checking {}",
                deadline.as_millis(),
                hook.as_str()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("plugin runtime stopped before replying to hook availability check".to_owned())
            }
        }
    }

    pub fn run_hook(&self, hook: PluginHook, value: &Value) -> Result<Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Command::RunHook {
                hook,
                value: value.clone(),
                reply,
            },
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
        )?;
        receive_result(receiver, CALLBACK_REPLY_DEADLINE, "run hook")
    }

    /// Runs a strict before-hook only if the plugin set is unchanged since
    /// [`Self::hook_token`] returned `token`.
    pub fn run_hook_checked(
        &self,
        hook: PluginHook,
        token: PluginHookToken,
        value: &Value,
    ) -> Result<Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Command::RunHookChecked {
                hook,
                token,
                value: value.clone(),
                reply,
            },
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
        )?;
        receive_result(receiver, CALLBACK_REPLY_DEADLINE, "run checked hook")
    }

    /// Runs an after-hook chain without turning an already successful remote
    /// mutation into an application error. A failing plugin is logged and the
    /// last successfully transformed value continues to the next plugin.
    #[must_use]
    pub fn run_hook_best_effort(&self, hook: PluginHook, value: &Value) -> Value {
        let fallback = value.clone();
        let (reply, receiver) = mpsc::sync_channel(1);
        if let Err(error) = self.enqueue(
            Command::RunHookBestEffort {
                hook,
                value: value.clone(),
                reply,
            },
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
        ) {
            tracing::warn!(?hook, %error, "plugin runtime unavailable while running an after hook");
            return fallback;
        }
        match receiver.recv_timeout(CALLBACK_REPLY_DEADLINE) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    ?hook,
                    timeout_ms = PLUGIN_CALLBACK_REPLY_TIMEOUT_MS,
                    %error,
                    "plugin runtime did not reply while running an after hook"
                );
                fallback
            }
        }
    }

    pub fn invoke_compose_button(
        &self,
        plugin_id: &str,
        button_id: &str,
        generation: u64,
        compose: Value,
    ) -> Result<Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Command::InvokeComposeButton {
                plugin_id: plugin_id.to_owned(),
                button_id: button_id.to_owned(),
                generation,
                compose,
                reply,
            },
            CALLBACK_REPLY_DEADLINE,
            PROMISE_DEADLINE,
        )?;
        receive_result(receiver, CALLBACK_REPLY_DEADLINE, "invoke compose button")
    }

    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::Release);
        let worker = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            let deadline = Instant::now() + SHUTDOWN_DEADLINE;
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(ACTOR_TICK);
            }
            if worker.is_finished() {
                if worker.join().is_err() {
                    tracing::warn!("plugin runtime thread panicked during shutdown");
                }
            } else {
                // The blocking reqwest backend cannot be interrupted in the
                // middle of an HTTP call. Do not make application exit wait
                // without bound for an unresponsive remote endpoint.
                tracing::warn!(
                    timeout_seconds = SHUTDOWN_DEADLINE.as_secs(),
                    "plugin runtime did not stop before the shutdown deadline"
                );
                drop(worker);
            }
        }
    }

    fn request_snapshot<F>(
        &self,
        command: F,
        reply_deadline: Duration,
        work_budget: Duration,
        operation: &'static str,
    ) -> Result<PluginSnapshot, String>
    where
        F: FnOnce(SnapshotReply) -> Command,
    {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(command(reply), reply_deadline, work_budget)?;
        receive_result(receiver, reply_deadline, operation)
    }

    fn enqueue(
        &self,
        command: Command,
        reply_deadline: Duration,
        work_budget: Duration,
    ) -> Result<(), String> {
        let margin = (reply_deadline / 10).min(Duration::from_secs(1));
        let finishes_by = Instant::now() + reply_deadline.saturating_sub(margin);
        let starts_by = finishes_by.checked_sub(work_budget).unwrap_or(finishes_by);
        match self.inner.sender.try_send(QueuedCommand {
            starts_by,
            finishes_by,
            command,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err("plugin runtime command queue is busy; try again".to_owned())
            }
            Err(TrySendError::Disconnected(_)) => Err("plugin runtime is not running".to_owned()),
        }
    }

    #[cfg(test)]
    pub(super) fn unload_plugin_with_deadline(
        &self,
        plugin_id: &str,
        deadline: Duration,
    ) -> Result<PluginSnapshot, String> {
        let plugin_id = plugin_id.to_owned();
        self.request_snapshot(
            move |reply| Command::UnloadPlugin { plugin_id, reply },
            deadline,
            Duration::ZERO,
            "unload plugin",
        )
    }

    #[cfg(test)]
    pub(super) fn enqueue_snapshot_for_test(
        &self,
        deadline: Duration,
    ) -> Result<Receiver<Result<PluginSnapshot, String>>, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Command::Snapshot(reply), deadline, Duration::ZERO)?;
        Ok(receiver)
    }

    #[cfg(test)]
    pub(super) fn run_hook_with_aggregate_deadline_for_test(
        &self,
        hook: PluginHook,
        value: &Value,
        aggregate_deadline: Duration,
    ) -> Result<Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let finishes_by = Instant::now() + aggregate_deadline;
        self.try_enqueue_for_test(
            Command::RunHook {
                hook,
                value: value.clone(),
                reply,
            },
            finishes_by,
        )?;
        receive_result(receiver, Duration::from_secs(2), "run test hook")
    }

    #[cfg(test)]
    pub(super) fn run_hook_best_effort_with_aggregate_deadline_for_test(
        &self,
        hook: PluginHook,
        value: &Value,
        aggregate_deadline: Duration,
    ) -> Result<Value, String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let finishes_by = Instant::now() + aggregate_deadline;
        self.try_enqueue_for_test(
            Command::RunHookBestEffort {
                hook,
                value: value.clone(),
                reply,
            },
            finishes_by,
        )?;
        receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| format!("test after-hook command did not reply: {error}"))
    }

    #[cfg(test)]
    fn try_enqueue_for_test(&self, command: Command, finishes_by: Instant) -> Result<(), String> {
        match self.inner.sender.try_send(QueuedCommand {
            starts_by: finishes_by,
            finishes_by,
            command,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err("plugin runtime command queue is busy; try again".to_owned())
            }
            Err(TrySendError::Disconnected(_)) => Err("plugin runtime is not running".to_owned()),
        }
    }
}

pub(super) fn receive_result<T>(
    receiver: Receiver<Result<T, String>>,
    deadline: Duration,
    operation: &str,
) -> Result<T, String> {
    match receiver.recv_timeout(deadline) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "plugin runtime timed out waiting for {operation} after {} ms",
            deadline.as_millis()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!(
            "plugin runtime stopped before replying to {operation}"
        )),
    }
}

enum Command {
    Snapshot(SnapshotReply),
    ReloadAll(SnapshotReply),
    ReloadPlugin {
        plugin_id: String,
        reply: SnapshotReply,
    },
    UnloadPlugin {
        plugin_id: String,
        reply: SnapshotReply,
    },
    HookToken {
        hook: PluginHook,
        reply: HookTokenReply,
    },
    RunHook {
        hook: PluginHook,
        value: Value,
        reply: ValueReply,
    },
    RunHookChecked {
        hook: PluginHook,
        token: PluginHookToken,
        value: Value,
        reply: ValueReply,
    },
    RunHookBestEffort {
        hook: PluginHook,
        value: Value,
        reply: SyncSender<Value>,
    },
    InvokeComposeButton {
        plugin_id: String,
        button_id: String,
        generation: u64,
        compose: Value,
        reply: ValueReply,
    },
}

struct QueuedCommand {
    starts_by: Instant,
    finishes_by: Instant,
    command: Command,
}

impl QueuedCommand {
    fn reject(self, message: &str) {
        match self.command {
            Command::Snapshot(reply) | Command::ReloadAll(reply) => {
                let _ = reply.send(Err(message.to_owned()));
            }
            Command::ReloadPlugin { reply, .. } | Command::UnloadPlugin { reply, .. } => {
                let _ = reply.send(Err(message.to_owned()));
            }
            Command::HookToken { reply, .. } => {
                let _ = reply.send(Err(message.to_owned()));
            }
            Command::RunHook { reply, .. }
            | Command::RunHookChecked { reply, .. }
            | Command::InvokeComposeButton { reply, .. } => {
                let _ = reply.send(Err(message.to_owned()));
            }
            Command::RunHookBestEffort { value, reply, .. } => {
                let _ = reply.send(value);
            }
        }
    }
}

struct PluginActor {
    directory: PathBuf,
    revision: u64,
    next_generation: u64,
    plugins: BTreeMap<String, PluginRecord>,
    directory_error: Option<String>,
    last_pumped_plugin: Option<String>,
    stop: Arc<AtomicBool>,
}

impl PluginActor {
    fn run(directory: PathBuf, receiver: Receiver<QueuedCommand>, stop: Arc<AtomicBool>) {
        install_rustls_provider();
        let mut actor = Self {
            directory,
            revision: 0,
            next_generation: 1,
            plugins: BTreeMap::new(),
            directory_error: None,
            last_pumped_plugin: None,
            stop,
        };

        if let Err(error) = actor.reload_all_inner(None) {
            tracing::error!(%error, "failed to initialize plugin directory");
            actor.directory_error = Some(error);
        }

        while !actor.stop.load(Ordering::Acquire) {
            match receiver.recv_timeout(ACTOR_TICK) {
                Ok(command) if Instant::now() >= command.starts_by => {
                    command.reject("plugin runtime command expired before execution");
                }
                Ok(command) => actor.handle(command.command, command.finishes_by),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    actor.pump_one_event_loop();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        while let Ok(command) = receiver.try_recv() {
            command.reject("plugin runtime is shutting down");
        }
        actor.unload_all();
    }

    fn handle(&mut self, command: Command, finishes_by: Instant) {
        match command {
            Command::Snapshot(reply) => {
                let result = self
                    .directory_error
                    .clone()
                    .map_or_else(|| Ok(self.snapshot_inner()), Err);
                let _ = reply.send(result);
            }
            Command::ReloadAll(reply) => {
                let result = self
                    .reload_all_inner(Some(finishes_by))
                    .map(|()| self.snapshot_inner());
                if result.is_ok() {
                    self.directory_error = None;
                }
                let _ = reply.send(result);
            }
            Command::ReloadPlugin { plugin_id, reply } => {
                let result = self
                    .reload_plugin_inner(&plugin_id)
                    .map(|()| self.snapshot_inner());
                let _ = reply.send(result);
            }
            Command::UnloadPlugin { plugin_id, reply } => {
                let result = self
                    .unload_plugin_inner(&plugin_id)
                    .map(|()| self.snapshot_inner());
                let _ = reply.send(result);
            }
            Command::HookToken { hook, reply } => {
                let token = self
                    .has_hook_inner(hook)
                    .then(|| PluginHookToken::new(self.revision, hook));
                let _ = reply.send(Ok(token));
            }
            Command::RunHook { hook, value, reply } => {
                let _ = reply.send(self.run_hook_inner(hook, value, finishes_by));
            }
            Command::RunHookChecked {
                hook,
                token,
                value,
                reply,
            } => {
                let result = if !token.matches(self.revision, hook) {
                    Err(format!(
                        "stale plugin hook token for {}; plugin set changed before invocation",
                        hook.as_str()
                    ))
                } else if !self.has_hook_inner(hook) {
                    Err(format!(
                        "plugin hook {} is no longer registered",
                        hook.as_str()
                    ))
                } else {
                    self.run_hook_inner(hook, value, finishes_by)
                };
                let _ = reply.send(result);
            }
            Command::RunHookBestEffort { hook, value, reply } => {
                let _ = reply.send(self.run_hook_best_effort_inner(hook, value, finishes_by));
            }
            Command::InvokeComposeButton {
                plugin_id,
                button_id,
                generation,
                compose,
                reply,
            } => {
                let _ = reply.send(self.invoke_compose_button_inner(
                    &plugin_id,
                    &button_id,
                    generation,
                    compose,
                    finishes_by,
                ));
            }
        }
    }

    fn reload_all_inner(&mut self, finishes_by: Option<Instant>) -> Result<(), String> {
        let discovered = discover_plugins(&self.directory)?;

        let removed = self
            .plugins
            .keys()
            .filter(|id| !discovered.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut record) = self.plugins.remove(&id) {
                record.stop_runtime();
            }
        }

        for (id, path) in &discovered {
            self.plugins
                .entry(id.clone())
                .and_modify(|record| record.path.clone_from(path))
                .or_insert_with(|| PluginRecord::new(id.clone(), path.clone()));
        }

        for id in discovered.keys() {
            if self.stop.load(Ordering::Acquire) {
                return Err("plugin runtime is shutting down".to_owned());
            }
            if finishes_by.is_some_and(|deadline| {
                deadline.saturating_duration_since(Instant::now()) < PROMISE_DEADLINE
            }) {
                return Err("plugin runtime command expired while reloading all plugins".to_owned());
            }
            self.reload_plugin_inner(id)?;
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn reload_plugin_inner(&mut self, plugin_id: &str) -> Result<(), String> {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);

        let record = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("plugin `{plugin_id}` was not discovered"))?;
        record.stop_runtime();
        record.generation = generation;
        record.version = None;
        record.error = None;
        record.state = PluginState::Unloaded;
        if !record.path.is_file() {
            let error = format!("plugin file `{}` does not exist", record.path.display());
            push_log(&record.logs, PluginLogLevel::Error, error.clone());
            record.error = Some(error);
            record.state = PluginState::Error;
            self.revision = self.revision.wrapping_add(1);
            return Ok(());
        }

        match PluginRuntime::load(
            plugin_id,
            &record.path,
            &self.directory,
            generation,
            Arc::clone(&record.logs),
            Arc::clone(&self.stop),
        ) {
            Ok(runtime) => {
                record.version = Some(runtime.version);
                record.runtime = Some(runtime);
                record.state = PluginState::Loaded;
            }
            Err(error) => {
                push_log(&record.logs, PluginLogLevel::Error, error.clone());
                record.error = Some(error);
                record.state = PluginState::Error;
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn unload_plugin_inner(&mut self, plugin_id: &str) -> Result<(), String> {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let record = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("plugin `{plugin_id}` was not discovered"))?;
        record.stop_runtime();
        record.generation = generation;
        record.state = PluginState::Unloaded;
        record.error = None;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn unload_all(&mut self) {
        for record in self.plugins.values_mut() {
            record.stop_runtime();
            record.state = PluginState::Unloaded;
        }
    }

    fn pump_one_event_loop(&mut self) {
        let next_id = self
            .plugins
            .iter()
            .filter(|(_, record)| record.runtime.is_some())
            .find(|(id, _)| {
                self.last_pumped_plugin
                    .as_ref()
                    .is_none_or(|last| id.as_str() > last.as_str())
            })
            .or_else(|| {
                self.plugins
                    .iter()
                    .find(|(_, record)| record.runtime.is_some())
            })
            .map(|(id, _)| id.clone());
        let Some(next_id) = next_id else {
            self.last_pumped_plugin = None;
            return;
        };
        self.last_pumped_plugin = Some(next_id.clone());

        let record = self
            .plugins
            .get_mut(&next_id)
            .expect("selected plugin must still exist");
        let result = record.runtime.as_mut().map(PluginRuntime::pump_event_loop);
        if let Some(Err(error)) = result {
            let message = error.to_string();
            push_log(&record.logs, PluginLogLevel::Error, message.clone());
            record.poison_runtime(message);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn has_hook_inner(&self, hook: PluginHook) -> bool {
        self.plugins.values().any(|record| {
            record
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.hooks.contains_key(&hook))
        })
    }

    fn run_hook_inner(
        &mut self,
        hook: PluginHook,
        mut value: Value,
        finishes_by: Instant,
    ) -> Result<Value, String> {
        let protected_metadata = protected_hook_metadata(&value);
        for (plugin_id, record) in &mut self.plugins {
            let Some(runtime) = record.runtime.as_mut() else {
                continue;
            };
            if !runtime.hooks.contains_key(&hook) {
                continue;
            }
            ensure_hook_callback_budget(finishes_by, hook)?;
            match runtime.invoke_hook(hook, &value, finishes_by) {
                Ok(mut next_value) => {
                    reassert_protected_hook_metadata(&mut next_value, &protected_metadata);
                    value = next_value;
                }
                Err(error) => {
                    let message = format!("plugin `{plugin_id}` {} failed: {error}", hook.as_str());
                    push_log(&record.logs, PluginLogLevel::Error, message.clone());
                    if error.poison_runtime {
                        record.poison_runtime(message.clone());
                        self.revision = self.revision.wrapping_add(1);
                    }
                    return Err(message);
                }
            }
        }
        Ok(value)
    }

    fn run_hook_best_effort_inner(
        &mut self,
        hook: PluginHook,
        mut value: Value,
        finishes_by: Instant,
    ) -> Value {
        let protected_metadata = protected_hook_metadata(&value);
        for (plugin_id, record) in &mut self.plugins {
            let Some(runtime) = record.runtime.as_mut() else {
                continue;
            };
            if !runtime.hooks.contains_key(&hook) {
                continue;
            }
            if let Err(error) = ensure_hook_callback_budget(finishes_by, hook) {
                tracing::warn!(hook = hook.as_str(), %error, "plugin after-hook chain reached its aggregate deadline");
                break;
            }
            match runtime.invoke_hook(hook, &value, finishes_by) {
                Ok(mut next_value) => {
                    reassert_protected_hook_metadata(&mut next_value, &protected_metadata);
                    value = next_value;
                }
                Err(error) => {
                    let message = format!("plugin `{plugin_id}` {} failed: {error}", hook.as_str());
                    push_log(&record.logs, PluginLogLevel::Error, message.clone());
                    tracing::warn!(%plugin_id, hook = hook.as_str(), %error, "plugin after hook failed");
                    if error.poison_runtime {
                        record.poison_runtime(message);
                        self.revision = self.revision.wrapping_add(1);
                    }
                }
            }
        }
        value
    }

    fn invoke_compose_button_inner(
        &mut self,
        plugin_id: &str,
        button_id: &str,
        generation: u64,
        compose: Value,
        finishes_by: Instant,
    ) -> Result<Value, String> {
        let record = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| format!("plugin `{plugin_id}` was not discovered"))?;
        if generation != record.generation {
            return Err(format!(
                "stale compose button generation {generation}; current generation is {}",
                record.generation
            ));
        }
        let runtime = record
            .runtime
            .as_mut()
            .ok_or_else(|| format!("plugin `{plugin_id}` is not loaded"))?;
        match runtime.invoke_compose_button(button_id, &compose, finishes_by) {
            Ok(value) => Ok(value),
            Err(error) => {
                let message =
                    format!("plugin `{plugin_id}` compose button `{button_id}` failed: {error}");
                push_log(&record.logs, PluginLogLevel::Error, message.clone());
                if error.poison_runtime {
                    record.poison_runtime(message.clone());
                    self.revision = self.revision.wrapping_add(1);
                }
                Err(message)
            }
        }
    }

    fn snapshot_inner(&self) -> PluginSnapshot {
        let plugins = self
            .plugins
            .values()
            .map(PluginRecord::info)
            .collect::<Vec<_>>();
        let compose_buttons = self
            .plugins
            .values()
            .filter_map(|record| record.runtime.as_ref())
            .flat_map(PluginRuntime::compose_button_descriptors)
            .collect::<Vec<_>>();
        PluginSnapshot {
            directory: self.directory.to_string_lossy().into_owned(),
            revision: self.revision,
            plugins,
            compose_buttons,
        }
    }
}

struct PluginRecord {
    id: String,
    path: PathBuf,
    version: Option<u32>,
    state: PluginState,
    generation: u64,
    error: Option<String>,
    logs: SharedLogs,
    runtime: Option<PluginRuntime>,
}

impl PluginRecord {
    fn new(id: String, path: PathBuf) -> Self {
        Self {
            id,
            path,
            version: None,
            state: PluginState::Unloaded,
            generation: 0,
            error: None,
            logs: Arc::new(Mutex::new(VecDeque::new())),
            runtime: None,
        }
    }

    fn stop_runtime(&mut self) {
        if let Some(mut runtime) = self.runtime.take() {
            boa_runtime::interval::clear_all(&mut runtime.context);
        }
    }

    fn poison_runtime(&mut self, message: String) {
        self.stop_runtime();
        self.generation = self.generation.wrapping_add(1);
        self.state = PluginState::Error;
        self.error = Some(message);
    }

    fn info(&self) -> PluginInfo {
        let logs = self
            .logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect();
        PluginInfo {
            id: self.id.clone(),
            file_name: self
                .path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            path: self.path.to_string_lossy().into_owned(),
            version: self.version,
            state: self.state,
            generation: self.generation,
            error: self.error.clone(),
            logs,
        }
    }
}

struct PluginRuntime {
    context: Context,
    executor: Rc<TurnJobExecutor>,
    default_export: JsObject,
    hooks: HashMap<PluginHook, JsObject>,
    compose_buttons: Vec<RegisteredComposeButton>,
    version: u32,
    stop: Arc<AtomicBool>,
    background_job_start: Option<u64>,
}

struct RegisteredComposeButton {
    descriptor: ComposeButtonDescriptor,
    owner: JsObject,
    on_click: JsObject,
}

impl PluginRuntime {
    fn load(
        plugin_id: &str,
        path: &Path,
        directory: &Path,
        generation: u64,
        logs: SharedLogs,
        stop: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let loader = Rc::new(SimpleModuleLoader::new(directory).map_err(js_error_string)?);
        let executor = Rc::new(TurnJobExecutor::with_stop(Arc::clone(&stop)));
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .module_loader(loader.clone())
            .build()
            .map_err(js_error_string)?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(LOOP_ITERATION_LIMIT);

        boa_runtime::register(
            (
                ConsoleExtension(PluginConsoleLogger { logs }),
                FetchExtension(
                    BoundedReqwestFetcher::new()
                        .map_err(|error| format!("failed to create plugin HTTP client: {error}"))?,
                ),
            ),
            None,
            &mut context,
        )
        .map_err(js_error_string)?;

        let canonical_path = path
            .canonicalize()
            .map_err(|error| format!("failed to resolve `{}`: {error}", path.display()))?;
        let source = Source::from_filepath(&canonical_path)
            .map_err(|error| format!("failed to read `{}`: {error}", canonical_path.display()))?;
        let module = Module::parse(source, None, &mut context).map_err(js_error_string)?;
        // Root modules are not cached automatically by SimpleModuleLoader.
        loader.insert(canonical_path, module.clone());
        let evaluation = module.load_link_evaluate(&mut context);
        let evaluation_started_jobs = executor.completed_jobs();
        await_promise(
            &evaluation,
            &mut context,
            &executor,
            &stop,
            evaluation_started_jobs,
            Instant::now() + PROMISE_DEADLINE,
        )
        .map_err(|error| format!("module evaluation failed: {error}"))?;

        let default_export = module
            .get_value(js_string!("default"), &mut context)
            .map_err(js_error_string)?
            .as_object()
            .ok_or_else(|| "default export must be an object".to_owned())?;

        let version_value = default_export
            .get(js_string!("version"), &mut context)
            .map_err(js_error_string)?;
        let version_number = version_value
            .as_number()
            .ok_or_else(|| "default export.version must be the number 1".to_owned())?;
        if !version_number.is_finite()
            || version_number.fract() != 0.0
            || version_number != f64::from(SUPPORTED_PLUGIN_VERSION)
        {
            return Err(format!(
                "unsupported plugin version {version_number}; expected {SUPPORTED_PLUGIN_VERSION}"
            ));
        }

        let hooks = extract_hooks(&default_export, &mut context)?;
        let compose_buttons =
            extract_compose_buttons(plugin_id, generation, &default_export, &mut context)?;

        Ok(Self {
            context,
            executor,
            default_export,
            hooks,
            compose_buttons,
            version: SUPPORTED_PLUGIN_VERSION,
            stop,
            background_job_start: None,
        })
    }

    fn invoke_hook(
        &mut self,
        hook: PluginHook,
        value: &Value,
        finishes_by: Instant,
    ) -> Result<Value, PluginExecutionError> {
        let function = self.hooks.get(&hook).cloned().ok_or_else(|| {
            PluginExecutionError::ordinary(format!("hook `{}` is not registered", hook.as_str()))
        })?;
        let result = invoke_json_function(
            &function,
            &self.default_export,
            value,
            &mut self.context,
            &self.executor,
            &self.stop,
            finishes_by,
        )?;
        if !result.is_object() {
            return Err(PluginExecutionError::ordinary(format!(
                "hook `{}` must return an object",
                hook.as_str()
            )));
        }
        Ok(result)
    }

    fn invoke_compose_button(
        &mut self,
        button_id: &str,
        value: &Value,
        finishes_by: Instant,
    ) -> Result<Value, PluginExecutionError> {
        let button = self
            .compose_buttons
            .iter()
            .find(|button| button.descriptor.button_id == button_id)
            .ok_or_else(|| {
                PluginExecutionError::ordinary(format!(
                    "compose button `{button_id}` is not registered"
                ))
            })?;
        let result = invoke_json_function(
            &button.on_click,
            &button.owner,
            value,
            &mut self.context,
            &self.executor,
            &self.stop,
            finishes_by,
        )?;
        if !result.is_object() {
            return Err(PluginExecutionError::ordinary(
                "compose button onClick must return an object",
            ));
        }
        Ok(result)
    }

    fn pump_event_loop(&mut self) -> Result<(), PluginExecutionError> {
        let before = self.executor.completed_jobs();
        if let Err(error) = run_jobs_with_control(
            &mut self.context,
            &self.executor,
            Instant::now() + PROMISE_DEADLINE,
        ) {
            return Err(PluginExecutionError {
                message: format!("background job failed: {error}"),
                poison_runtime: error.poison_runtime,
            });
        }
        if self.executor.has_pending_immediate_jobs() {
            let start = *self.background_job_start.get_or_insert(before);
            if self.executor.completed_jobs().wrapping_sub(start) >= MAX_JOBS_PER_CALLBACK {
                return Err(PluginExecutionError::control_limit(format!(
                    "background job queue exceeded the limit of {MAX_JOBS_PER_CALLBACK} jobs"
                )));
            }
        } else {
            self.background_job_start = None;
        }
        Ok(())
    }

    fn compose_button_descriptors(&self) -> impl Iterator<Item = ComposeButtonDescriptor> + '_ {
        self.compose_buttons
            .iter()
            .map(|button| button.descriptor.clone())
    }
}

fn extract_hooks(
    default_export: &JsObject,
    context: &mut Context,
) -> Result<HashMap<PluginHook, JsObject>, String> {
    let mut hooks = HashMap::new();
    for hook in PluginHook::ALL {
        let value = default_export
            .get(JsString::from(hook.as_str()), context)
            .map_err(js_error_string)?;
        if value.is_undefined() {
            continue;
        }
        let function = value
            .as_object()
            .filter(|object| object.is_callable())
            .ok_or_else(|| format!("default export.{} must be a function", hook.as_str()))?;
        hooks.insert(hook, function);
    }
    Ok(hooks)
}

fn extract_compose_buttons(
    plugin_id: &str,
    generation: u64,
    default_export: &JsObject,
    context: &mut Context,
) -> Result<Vec<RegisteredComposeButton>, String> {
    let plural = default_export
        .get(js_string!("registerComposeButtons"), context)
        .map_err(js_error_string)?;
    let (value, compatibility_alias) = if plural.is_undefined() {
        (
            default_export
                .get(js_string!("registerComposeButton"), context)
                .map_err(js_error_string)?,
            true,
        )
    } else {
        (plural, false)
    };
    if value.is_undefined() {
        return Ok(Vec::new());
    }
    let container = value
        .as_object()
        .ok_or_else(|| "default export.registerComposeButtons must be an array".to_owned())?;
    let owners = if container.is_array() {
        let array = JsArray::from_object(container).map_err(js_error_string)?;
        let length = array.length(context).map_err(js_error_string)?;
        let mut owners = Vec::with_capacity(length as usize);
        for index in 0..length {
            owners.push(
                array
                    .get(index, context)
                    .map_err(js_error_string)?
                    .as_object()
                    .ok_or_else(|| format!("registerComposeButtons[{index}] must be an object"))?,
            );
        }
        owners
    } else if compatibility_alias {
        vec![container]
    } else {
        return Err("default export.registerComposeButtons must be an array".to_owned());
    };

    let mut buttons = Vec::with_capacity(owners.len());
    for (index, owner) in owners.into_iter().enumerate() {
        let icon = required_string_property(&owner, "icon", context)
            .map_err(|error| format!("registerComposeButtons[{index}].{error}"))?;
        let label = optional_string_property(&owner, "label", context)
            .map_err(|error| format!("registerComposeButtons[{index}].{error}"))?;
        let on_click = owner
            .get(js_string!("onClick"), context)
            .map_err(js_error_string)?
            .as_object()
            .filter(|object| object.is_callable())
            .ok_or_else(|| format!("registerComposeButtons[{index}].onClick must be a function"))?;
        buttons.push(RegisteredComposeButton {
            descriptor: ComposeButtonDescriptor {
                plugin_id: plugin_id.to_owned(),
                button_id: index.to_string(),
                generation,
                icon,
                label,
            },
            owner,
            on_click,
        });
    }
    Ok(buttons)
}

fn required_string_property(
    object: &JsObject,
    name: &str,
    context: &mut Context,
) -> Result<String, String> {
    let value = object
        .get(JsString::from(name), context)
        .map_err(js_error_string)?;
    value
        .as_string()
        .map(|value| value.to_std_string_escaped())
        .ok_or_else(|| format!("{name} must be a string"))
}

fn optional_string_property(
    object: &JsObject,
    name: &str,
    context: &mut Context,
) -> Result<Option<String>, String> {
    let value = object
        .get(JsString::from(name), context)
        .map_err(js_error_string)?;
    if value.is_undefined() {
        return Ok(None);
    }
    value
        .as_string()
        .map(|value| Some(value.to_std_string_escaped()))
        .ok_or_else(|| format!("{name} must be a string when provided"))
}

fn invoke_json_function(
    function: &JsObject,
    owner: &JsObject,
    value: &Value,
    context: &mut Context,
    executor: &TurnJobExecutor,
    stop: &AtomicBool,
    command_deadline: Instant,
) -> Result<Value, PluginExecutionError> {
    let deadline = (Instant::now() + PROMISE_DEADLINE).min(command_deadline);
    let started_jobs = executor.completed_jobs();
    check_runtime_control(executor, stop, started_jobs, deadline, false)?;
    let argument = JsValue::from_json(value, context).map_err(plugin_execution_error)?;
    let returned = match function.call(&owner.clone().into(), &[argument], context) {
        Ok(returned) => returned,
        Err(error) => {
            // A throwing callback can still have queued microtasks before the
            // throw. Run the host checkpoint so an infinite chain is detected
            // and poisons the runtime instead of escaping to the background.
            run_microtask_checkpoint_with_limits(context, executor, stop, started_jobs, deadline)?;
            return Err(plugin_execution_error(error));
        }
    };
    check_runtime_control(executor, stop, started_jobs, deadline, false)?;
    let promise = match JsPromise::resolve(returned, context) {
        Ok(promise) => promise,
        Err(error) => {
            run_microtask_checkpoint_with_limits(context, executor, stop, started_jobs, deadline)?;
            return Err(plugin_execution_error(error));
        }
    };
    let resolved = await_promise(&promise, context, executor, stop, started_jobs, deadline)?;
    check_runtime_control(executor, stop, started_jobs, deadline, false)?;
    resolved
        .to_json(context)
        .map_err(plugin_execution_error)?
        .ok_or_else(|| {
            PluginExecutionError::ordinary("plugin callback returned undefined or a non-JSON value")
        })
}

fn await_promise(
    promise: &JsPromise,
    context: &mut Context,
    executor: &TurnJobExecutor,
    stop: &AtomicBool,
    started_jobs: u64,
    deadline: Instant,
) -> Result<JsValue, PluginExecutionError> {
    loop {
        match promise.state() {
            PromiseState::Pending => {
                check_runtime_control(executor, stop, started_jobs, deadline, true)?;
                run_jobs_with_control(context, executor, deadline)?;
                if matches!(promise.state(), PromiseState::Pending) {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            PromiseState::Fulfilled(value) => {
                run_microtask_checkpoint_with_limits(
                    context,
                    executor,
                    stop,
                    started_jobs,
                    deadline,
                )?;
                return Ok(value);
            }
            PromiseState::Rejected(reason) => {
                run_microtask_checkpoint_with_limits(
                    context,
                    executor,
                    stop,
                    started_jobs,
                    deadline,
                )?;
                return Err(PluginExecutionError::ordinary(js_error_string(
                    JsError::from_opaque(reason),
                )));
            }
        }
    }
}

fn run_microtask_checkpoint_with_limits(
    context: &mut Context,
    executor: &TurnJobExecutor,
    stop: &AtomicBool,
    started_jobs: u64,
    deadline: Instant,
) -> Result<(), PluginExecutionError> {
    check_runtime_control(executor, stop, started_jobs, deadline, false)?;
    while executor.has_pending_immediate_jobs() {
        check_runtime_control(executor, stop, started_jobs, deadline, true)?;
        run_jobs_with_control(context, executor, deadline)?;
    }
    Ok(())
}

fn run_jobs_with_control(
    context: &mut Context,
    executor: &TurnJobExecutor,
    deadline: Instant,
) -> Result<(), PluginExecutionError> {
    executor.set_execution_deadline(deadline);
    let result = context.run_jobs();
    executor.clear_execution_deadline();
    match executor.take_control_error() {
        Some(JobControlError::Deadline) => Err(PluginExecutionError::control_limit(
            "plugin job turn could not finish before its absolute deadline",
        )),
        Some(JobControlError::Stopped) => Err(PluginExecutionError::control_limit(
            "plugin runtime is shutting down",
        )),
        None => result.map_err(plugin_execution_error),
    }
}

fn check_runtime_control(
    executor: &TurnJobExecutor,
    stop: &AtomicBool,
    started_jobs: u64,
    deadline: Instant,
    more_work_required: bool,
) -> Result<(), PluginExecutionError> {
    if stop.load(Ordering::Acquire) {
        return Err(PluginExecutionError::control_limit(
            "plugin runtime is shutting down",
        ));
    }
    if Instant::now() >= deadline {
        return Err(PluginExecutionError::control_limit(format!(
            "promise or microtask checkpoint did not settle within {} seconds",
            PROMISE_DEADLINE.as_secs()
        )));
    }
    if more_work_required
        && executor.completed_jobs().wrapping_sub(started_jobs) >= MAX_JOBS_PER_CALLBACK
    {
        return Err(PluginExecutionError::control_limit(format!(
            "job queue exceeded the limit of {MAX_JOBS_PER_CALLBACK} jobs"
        )));
    }
    Ok(())
}

fn ensure_hook_callback_budget(finishes_by: Instant, hook: PluginHook) -> Result<(), String> {
    if finishes_by.saturating_duration_since(Instant::now()) < PROMISE_DEADLINE {
        return Err(format!(
            "plugin hook chain {} reached its aggregate deadline before the next callback",
            hook.as_str()
        ));
    }
    Ok(())
}

fn plugin_execution_error(error: JsError) -> PluginExecutionError {
    let poison_runtime = matches!(error.as_engine(), Some(EngineError::RuntimeLimit(_)));
    let message = js_error_string(error);
    if poison_runtime {
        PluginExecutionError::control_limit(message)
    } else {
        PluginExecutionError::ordinary(message)
    }
}

fn protected_hook_metadata(value: &Value) -> [Option<Value>; PROTECTED_HOOK_METADATA.len()] {
    PROTECTED_HOOK_METADATA.map(|key| value.get(key).cloned())
}

fn reassert_protected_hook_metadata(
    value: &mut Value,
    protected: &[Option<Value>; PROTECTED_HOOK_METADATA.len()],
) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for (key, original) in PROTECTED_HOOK_METADATA.into_iter().zip(protected) {
        if let Some(original) = original {
            object.insert(key.to_owned(), original.clone());
        } else {
            object.remove(key);
        }
    }
}

fn discover_plugins(directory: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    fs::create_dir_all(directory).map_err(|error| {
        format!(
            "failed to create plugin directory `{}`: {error}",
            directory.display()
        )
    })?;
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "failed to read plugin directory `{}`: {error}",
            directory.display()
        )
    })?;

    let mut plugins = BTreeMap::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to inspect plugin directory: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect `{}`: {error}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("js") || extension.eq_ignore_ascii_case("mjs")
            });
        if !supported {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        plugins.insert(id, path);
    }
    Ok(plugins)
}

#[derive(Debug)]
struct PluginConsoleLogger {
    logs: SharedLogs,
}

impl boa_engine::Finalize for PluginConsoleLogger {}

// SAFETY: `logs` contains only ordinary Rust synchronization and owned data;
// it cannot contain a Boa GC handle and therefore has nothing to trace.
unsafe impl boa_engine::Trace for PluginConsoleLogger {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}

    unsafe fn trace_non_roots(&self) {}

    fn run_finalizer(&self) {
        boa_engine::Finalize::finalize(self);
    }
}

impl PluginConsoleLogger {
    fn write(&self, level: PluginLogLevel, message: String) {
        push_log(&self.logs, level, message);
    }
}

impl Logger for PluginConsoleLogger {
    fn trace(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Trace, message);
        Ok(())
    }

    fn debug(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Debug, message);
        Ok(())
    }

    fn log(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Log, message);
        Ok(())
    }

    fn info(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Info, message);
        Ok(())
    }

    fn warn(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Warn, message);
        Ok(())
    }

    fn error(
        &self,
        message: String,
        _state: &ConsoleState,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        self.write(PluginLogLevel::Error, message);
        Ok(())
    }
}

fn push_log(logs: &SharedLogs, level: PluginLogLevel, message: String) {
    let mut logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    while logs.len() >= MAX_LOG_ENTRIES {
        logs.pop_front();
    }
    logs.push_back(PluginLogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level,
        message,
    });
}

fn install_rustls_provider() {
    static INSTALL_PROVIDER: Once = Once::new();
    INSTALL_PROVIDER.call_once(|| {
        // Another networking stack may have installed a process-wide provider
        // first. AlreadyInstalled is therefore a successful outcome for us.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn js_error_string(error: JsError) -> String {
    error.to_string()
}
