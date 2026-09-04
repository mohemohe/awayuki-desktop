use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use super::manager::receive_result;
use super::{PluginHook, PluginManager, PluginState};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "awayuki-plugin-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test plugin directory should be unique");
        Self { path }
    }

    fn write(&self, file_name: &str, source: &str) {
        fs::write(self.path.join(file_name), source).expect("plugin source should be writable");
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn start(directory: &TestDirectory) -> PluginManager {
    PluginManager::start(directory.path().to_path_buf()).expect("plugin actor should start")
}

#[test]
fn manager_reply_wait_is_bounded_and_reports_a_timeout() {
    let (sender, receiver) = mpsc::sync_channel::<Result<(), String>>(1);
    let started = Instant::now();

    let error = receive_result(receiver, Duration::from_millis(20), "test command")
        .expect_err("an actor that never replies must time out");

    assert!(error.contains("timed out waiting for test command"));
    assert!(error.contains("20 ms"));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(sender);
}

#[test]
fn hook_availability_failure_is_not_reported_as_hook_absence() {
    let directory = TestDirectory::new("hook-availability-stopped");
    let manager = start(&directory);
    manager
        .snapshot()
        .expect("empty plugin directory should initialize");
    manager.shutdown();

    let error = manager
        .has_hook(PluginHook::BeforeCreatePost)
        .expect_err("a stopped actor must not look like an absent strict hook");
    assert!(error.contains("plugin runtime is not running"));
}

#[test]
fn loads_the_exact_sample_and_uses_callback_return_values() {
    let directory = TestDirectory::new("exact-sample");
    directory.write(
        "sample.mjs",
        include_str!("../../docs/examples/plugins/sample.mjs"),
    );

    let manager = start(&directory);
    let snapshot = manager.snapshot().expect("snapshot should load");
    assert_eq!(snapshot.plugins.len(), 1);
    assert_eq!(snapshot.plugins[0].state, PluginState::Loaded);
    assert_eq!(snapshot.plugins[0].version, Some(1));
    assert_eq!(snapshot.compose_buttons.len(), 1);
    assert_eq!(snapshot.compose_buttons[0].icon, "🥹\u{200b}");

    let transformed = manager
        .run_hook(
            PluginHook::BeforeCreatePost,
            &json!({"visibility": "public", "text": "これは内緒"}),
        )
        .expect("hook should run");
    assert_eq!(transformed["visibility"], "unlisted");

    let button = &snapshot.compose_buttons[0];
    let draft = manager
        .invoke_compose_button(
            &button.plugin_id,
            &button.button_id,
            button.generation,
            json!({"text": "hello", "cw_title": ""}),
        )
        .expect("compose callback should run");
    assert_eq!(draft["cw_title"], "ぴえん");
    manager.shutdown();
}

#[test]
fn pumps_queue_microtask_and_a_timeout_backed_promise() {
    let directory = TestDirectory::new("async-turns");
    directory.write(
        "async.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: async (obj) => {
    await new Promise((resolve) => {
      queueMicrotask(() => { obj.microtask = true; });
      setTimeout(resolve, 5);
    });
    obj.timeout = true;
    return obj;
  },
};
"#,
    );

    let manager = start(&directory);
    let transformed = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({"text": "hello"}))
        .expect("async hook should settle");
    assert_eq!(transformed["microtask"], true);
    assert_eq!(transformed["timeout"], true);
    manager.shutdown();
}

#[test]
fn drains_finite_microtasks_across_multiple_bounded_turns() {
    let directory = TestDirectory::new("finite-microtasks");
    directory.write(
        "finite.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: async (obj) => {
    await new Promise((resolve) => {
      let remaining = 512;
      function again() {
        obj.microtasks = (obj.microtasks ?? 0) + 1;
        remaining -= 1;
        if (remaining === 0) {
          resolve();
        } else {
          queueMicrotask(again);
        }
      }
      queueMicrotask(again);
    });
    return obj;
  },
};
"#,
    );

    let manager = start(&directory);
    let transformed = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect("finite microtask chain should drain");
    assert_eq!(transformed["microtasks"], 512);
    manager.shutdown();
}

#[test]
fn pumps_set_interval_until_clear_interval() {
    let directory = TestDirectory::new("interval");
    directory.write(
        "interval.js",
        r#"
let ticks = 0;
const interval = setInterval(() => {
  ticks += 1;
  console.log(`interval-${ticks}`);
  if (ticks === 3) clearInterval(interval);
}, 1);
export default { version: 1 };
"#,
    );

    let manager = start(&directory);
    manager.snapshot().expect("plugin should load");
    thread::sleep(Duration::from_millis(100));
    let snapshot = manager
        .snapshot()
        .expect("interval logs should be readable");
    let messages = snapshot.plugins[0]
        .logs
        .iter()
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages, ["interval-1", "interval-2", "interval-3"]);
    manager.shutdown();
}

#[test]
fn bounds_console_logs_to_the_latest_five_hundred_entries() {
    let directory = TestDirectory::new("log-cap");
    directory.write(
        "logs.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: (obj) => {
    for (let index = 0; index < 600; index += 1) console.log(`line-${index}`);
    return obj;
  },
};
"#,
    );

    let manager = start(&directory);
    manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect("logging hook should return");
    let snapshot = manager.snapshot().expect("bounded logs should be readable");
    assert_eq!(snapshot.plugins[0].logs.len(), 500);
    assert_eq!(snapshot.plugins[0].logs[0].message, "line-100");
    assert_eq!(snapshot.plugins[0].logs[499].message, "line-599");
    manager.shutdown();
}

#[test]
fn poisons_a_runtime_that_self_reschedules_microtasks() {
    let directory = TestDirectory::new("infinite-microtasks");
    directory.write(
        "infinite.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: (obj) => {
    setTimeout(() => console.log("late timer must not run"), 1);
    function again() { queueMicrotask(again); }
    queueMicrotask(again);
    return obj;
  },
  registerComposeButtons: [{ icon: "X", onClick: (obj) => obj }],
};
"#,
    );

    let manager = start(&directory);
    let started = Instant::now();
    let error = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect_err("self-rescheduling microtasks must hit the job limit");
    assert!(error.contains("job queue exceeded the limit"));
    assert!(started.elapsed() < Duration::from_secs(2));

    let snapshot = manager
        .snapshot()
        .expect("poisoned state should be visible");
    assert_eq!(snapshot.plugins[0].state, PluginState::Error);
    assert!(snapshot.plugins[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("job queue exceeded the limit")));
    assert!(snapshot.compose_buttons.is_empty());
    assert!(!manager
        .has_hook(PluginHook::BeforeCreatePost)
        .expect("poisoned plugin has no registered hook"));

    thread::sleep(Duration::from_millis(30));
    let after_wait = manager.snapshot().expect("logs should remain readable");
    assert!(!after_wait.plugins[0]
        .logs
        .iter()
        .any(|entry| entry.message == "late timer must not run"));
    manager.shutdown();
}

#[test]
fn poisons_a_runtime_that_exceeds_the_synchronous_loop_limit() {
    let directory = TestDirectory::new("sync-loop-limit");
    directory.write(
        "loop.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: () => { while (true) {} },
  registerComposeButtons: [{ icon: "X", onClick: (obj) => obj }],
};
"#,
    );

    let manager = start(&directory);
    let started = Instant::now();
    let error = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect_err("an infinite synchronous loop must hit Boa's runtime limit");
    assert!(error.contains("RuntimeLimitError"));
    assert!(started.elapsed() < Duration::from_secs(2));

    let snapshot = manager
        .snapshot()
        .expect("poisoned state should be visible");
    assert_eq!(snapshot.plugins[0].state, PluginState::Error);
    assert!(snapshot.compose_buttons.is_empty());
    assert!(!manager
        .has_hook(PluginHook::BeforeCreatePost)
        .expect("poisoned plugin should expose no hooks"));
    manager.shutdown();
}

#[test]
fn resolves_fetch_with_the_blocking_reqwest_backend() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
    listener
        .set_nonblocking(true)
        .expect("loopback listener should become nonblocking");
    let address = listener
        .local_addr()
        .expect("listener should have an address");
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("test stream timeout should be set");
                    let mut request = [0_u8; 2048];
                    let _ = stream.read(&mut request);
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfetched",
                        )
                        .expect("test response should be written");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        panic!("plugin fetch did not reach the loopback server");
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("loopback accept failed: {error}"),
            }
        }
    });

    let directory = TestDirectory::new("fetch");
    let source = r#"
export default {
  version: 1,
  beforeCreatePost: async (obj) => {
    console.log("fetching loopback");
    const response = await fetch("__URL__");
    obj.fetched = await response.text();
    return obj;
  },
};
"#
    .replace("__URL__", &format!("http://{address}/value"));
    directory.write("fetch.js", &source);

    let manager = start(&directory);
    let transformed = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect("fetch hook should settle");
    assert_eq!(transformed["fetched"], "fetched");
    let snapshot = manager.snapshot().expect("logs should be readable");
    assert!(snapshot.plugins[0]
        .logs
        .iter()
        .any(|entry| entry.message == "fetching loopback"));
    manager.shutdown();
    server.join().expect("loopback server should finish");
}

#[test]
fn delayed_fetch_is_bounded_and_busy_hook_checks_return_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should have an address");
    let (accepted_sender, accepted_receiver) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("plugin should connect");
        accepted_sender
            .send(())
            .expect("test should observe the accepted request");
        thread::sleep(Duration::from_millis(600));
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nlate",
        );
    });

    let directory = TestDirectory::new("fetch-timeout");
    let source = r#"
export default {
  version: 1,
  beforeCreatePost: async (obj) => {
    const response = await fetch("__URL__");
    obj.result = await response.text();
    return obj;
  },
};
"#
    .replace("__URL__", &format!("http://{address}/slow"));
    directory.write("slow.js", &source);
    let manager = start(&directory);
    assert_eq!(
        manager.snapshot().expect("plugin should load").plugins[0].state,
        PluginState::Loaded
    );

    let hook_manager = manager.clone();
    let started = Instant::now();
    let hook =
        thread::spawn(move || hook_manager.run_hook(PluginHook::BeforeCreatePost, &json!({})));
    accepted_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("fetch should reach the delayed server");

    let availability =
        manager.has_hook_with_deadline(PluginHook::BeforeCreatePost, Duration::from_millis(20));
    assert!(availability.unwrap_err().contains("timed out"));
    let unload = manager.unload_plugin_with_deadline("slow.js", Duration::from_millis(20));
    assert!(
        unload.unwrap_err().contains("timed out"),
        "an unload that cannot start before its deadline must fail at the caller"
    );

    let mut queued_snapshots = Vec::new();
    let queue_error = loop {
        match manager.enqueue_snapshot_for_test(Duration::from_secs(1)) {
            Ok(receiver) => queued_snapshots.push(receiver),
            Err(error) => break error,
        }
    };
    assert!(queue_error.contains("command queue is busy"));
    assert!(!queued_snapshots.is_empty());

    let hook_error = hook
        .join()
        .expect("hook caller thread should finish")
        .expect_err("delayed response must hit the fetch timeout");
    assert!(
        hook_error.to_ascii_lowercase().contains("timed out"),
        "unexpected delayed-fetch error: {hook_error}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    for receiver in queued_snapshots {
        let snapshot = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("admitted snapshot should receive a bounded reply")
            .expect("admitted snapshot should succeed after the fetch returns");
        assert_eq!(snapshot.plugins[0].state, PluginState::Loaded);
    }
    assert_eq!(
        manager.snapshot().expect("actor should recover").plugins[0].state,
        PluginState::Loaded,
        "an unload command that expired in the queue must never execute later"
    );
    assert!(manager
        .has_hook(PluginHook::BeforeCreatePost)
        .expect("normal availability check should work after timeout"));
    manager.shutdown();
    server.join().expect("delayed server should finish");
}

#[test]
fn queued_commands_take_priority_over_round_robin_background_fetches() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should have an address");
    let (accepted_sender, accepted_receiver) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("first plugin should connect");
        accepted_sender
            .send(())
            .expect("test should observe the background fetch");
        thread::sleep(Duration::from_millis(600));
        drop(stream);
    });

    let directory = TestDirectory::new("background-command-priority");
    let source = r#"
setTimeout(() => { fetch("__URL__").catch(() => {}); }, 1);
export default { version: 1 };
"#
    .replace("__URL__", &format!("http://{address}/slow"));
    directory.write("00-first.js", &source);
    directory.write("10-second.js", &source);

    let manager = start(&directory);
    manager.snapshot().expect("both plugins should load");
    accepted_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("the first round-robin runtime should begin its fetch");

    let started = Instant::now();
    let snapshot_receiver = manager
        .enqueue_snapshot_for_test(Duration::from_millis(400))
        .expect("quick command should be admitted");
    let snapshot = snapshot_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("snapshot should run before pumping the second runtime")
        .expect("snapshot command must not expire behind background work");

    assert_eq!(snapshot.plugins.len(), 2);
    assert!(started.elapsed() < Duration::from_millis(400));
    manager.shutdown();
    server.join().expect("background server should finish");
}

#[test]
fn unload_removes_buttons_and_rejects_the_stale_generation() {
    let directory = TestDirectory::new("unload");
    directory.write(
        "button.js",
        r#"
export default {
  version: 1,
  registerComposeButtons: [{ icon: "A", onClick: (obj) => obj }],
};
"#,
    );

    let manager = start(&directory);
    let loaded = manager.snapshot().expect("snapshot should load");
    let old_button = loaded.compose_buttons[0].clone();
    let unloaded = manager
        .unload_plugin(&old_button.plugin_id)
        .expect("unload should succeed");
    assert_eq!(unloaded.plugins[0].state, PluginState::Unloaded);
    assert!(unloaded.compose_buttons.is_empty());

    let stale = manager.invoke_compose_button(
        &old_button.plugin_id,
        &old_button.button_id,
        old_button.generation,
        json!({}),
    );
    assert!(stale
        .unwrap_err()
        .contains("stale compose button generation"));

    let reloaded = manager
        .reload_plugin(&old_button.plugin_id)
        .expect("reload should succeed");
    assert_eq!(reloaded.plugins[0].state, PluginState::Loaded);
    assert_ne!(
        reloaded.compose_buttons[0].generation,
        old_button.generation
    );
    manager.shutdown();
}

#[test]
fn missing_file_reload_drops_the_old_runtime_and_buttons() {
    let directory = TestDirectory::new("missing-reload");
    directory.write(
        "removed.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: (obj) => obj,
  registerComposeButtons: [{ icon: "R", onClick: (obj) => obj }],
};
"#,
    );

    let manager = start(&directory);
    let loaded = manager.snapshot().expect("plugin should initially load");
    let old_button = loaded.compose_buttons[0].clone();
    fs::remove_file(directory.path().join("removed.js"))
        .expect("test plugin file should be removed");

    let failed = manager
        .reload_plugin(&old_button.plugin_id)
        .expect("missing discovered file should return an error-state snapshot");
    assert_eq!(failed.plugins[0].state, PluginState::Error);
    assert!(failed.plugins[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("does not exist")));
    assert!(failed.compose_buttons.is_empty());
    assert!(!manager
        .has_hook(PluginHook::BeforeCreatePost)
        .expect("actor should remain responsive"));
    let stale = manager.invoke_compose_button(
        &old_button.plugin_id,
        &old_button.button_id,
        old_button.generation,
        json!({}),
    );
    assert!(stale
        .unwrap_err()
        .contains("stale compose button generation"));
    manager.shutdown();
}

#[test]
fn isolates_bad_files_and_chains_valid_plugins_in_filename_order() {
    let directory = TestDirectory::new("isolation-order");
    directory.write(
        "00-first.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: (obj) => { obj.order.push("first"); return obj; },
};
"#,
    );
    directory.write("10-syntax.js", "export default { version: 1,, };\n");
    directory.write("20-version.js", "export default { version: 2 };\n");
    directory.write(
        "30-second.js",
        r#"
export default {
  version: 1,
  beforeCreatePost: (obj) => { obj.order.push("second"); return obj; },
};
"#,
    );

    let manager = start(&directory);
    let snapshot = manager.snapshot().expect("file errors should be isolated");
    assert_eq!(snapshot.plugins.len(), 4);
    assert_eq!(
        snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.state == PluginState::Loaded)
            .count(),
        2
    );
    assert_eq!(
        snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.state == PluginState::Error)
            .count(),
        2
    );

    let transformed = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({"order": []}))
        .expect("valid plugins should still run");
    assert_eq!(transformed["order"], json!(["first", "second"]));
    manager.shutdown();
}

#[test]
fn reasserts_protected_metadata_between_plugins_and_on_the_final_value() {
    let directory = TestDirectory::new("protected-hook-metadata");
    directory.write(
        "00-spoofer.js",
        r#"
export default {
  version: 1,
  beforeBoost: (obj) => {
    obj._awayukiAction = "reblog";
    obj._awayukiActingAccountAcct = "mallory@example.net";
    obj.actingAccountAcct = "mallory@example.net";
    obj.operationId = "forged";
    return obj;
  },
};
"#,
    );
    directory.write(
        "10-observer.js",
        r#"
export default {
  version: 1,
  beforeBoost: (obj) => {
    obj.observed = {
      action: obj._awayukiAction,
      privateAcct: obj._awayukiActingAccountAcct,
      actingAcct: obj.actingAccountAcct,
      hasOperationId: Object.hasOwn(obj, "operationId"),
    };
    return obj;
  },
};
"#,
    );

    let manager = start(&directory);
    let transformed = manager
        .run_hook(
            PluginHook::BeforeBoost,
            &json!({
                "_awayukiAction": "unreblog",
                "_awayukiActingAccountAcct": "alice@example.com",
                "actingAccountAcct": "alice@example.com",
            }),
        )
        .expect("protected metadata chain should run");

    assert_eq!(transformed["_awayukiAction"], "unreblog");
    assert_eq!(
        transformed["_awayukiActingAccountAcct"],
        "alice@example.com"
    );
    assert_eq!(transformed["actingAccountAcct"], "alice@example.com");
    assert!(transformed.get("operationId").is_none());
    assert_eq!(transformed["observed"]["action"], "unreblog");
    assert_eq!(transformed["observed"]["privateAcct"], "alice@example.com");
    assert_eq!(transformed["observed"]["actingAcct"], "alice@example.com");
    assert_eq!(transformed["observed"]["hasOperationId"], false);
    manager.shutdown();
}

#[test]
fn aggregate_hook_deadline_stops_strict_and_best_effort_chains_before_later_plugins() {
    let directory = TestDirectory::new("aggregate-hook-deadline");
    directory.write(
        "00-first.js",
        r#"
const delay = () => new Promise((resolve) => setTimeout(resolve, 200));
export default {
  version: 1,
  beforeBookmark: async (obj) => {
    await delay();
    obj.order.push("before-first");
    return obj;
  },
  afterBookmark: async (obj) => {
    await delay();
    obj.order.push("after-first");
    return obj;
  },
};
"#,
    );
    directory.write(
        "10-must-not-run.js",
        r#"
export default {
  version: 1,
  beforeBookmark: (obj) => {
    console.log("late before callback ran");
    obj.order.push("before-late");
    return obj;
  },
  afterBookmark: (obj) => {
    console.log("late after callback ran");
    obj.order.push("after-late");
    return obj;
  },
};
"#,
    );

    let manager = start(&directory);
    manager.snapshot().expect("plugins should load");
    // The first callback starts with slightly more than its 30 second budget,
    // then consumes enough time that the next callback can no longer fit.
    let aggregate_deadline = Duration::from_millis(30_100);
    let started = Instant::now();
    let strict_error = manager
        .run_hook_with_aggregate_deadline_for_test(
            PluginHook::BeforeBookmark,
            &json!({"order": []}),
            aggregate_deadline,
        )
        .expect_err("strict chain must fail before starting the later plugin");
    assert!(strict_error.contains("aggregate deadline"));
    assert!(started.elapsed() < Duration::from_secs(2));

    let after = manager
        .run_hook_best_effort_with_aggregate_deadline_for_test(
            PluginHook::AfterBookmark,
            &json!({"order": []}),
            aggregate_deadline,
        )
        .expect("best-effort chain should return its last safe value");
    assert_eq!(after["order"], json!(["after-first"]));

    let snapshot = manager.snapshot().expect("logs should remain readable");
    let late_plugin = snapshot
        .plugins
        .iter()
        .find(|plugin| plugin.id == "10-must-not-run.js")
        .expect("later plugin should remain discovered");
    assert!(!late_plugin.logs.iter().any(|entry| {
        entry.message == "late before callback ran" || entry.message == "late after callback ran"
    }));
    manager.shutdown();
}

#[test]
fn checked_hook_rejects_a_token_made_stale_by_unload_or_reload() {
    let directory = TestDirectory::new("checked-hook-token");
    directory.write(
        "checked.js",
        r#"
export default {
  version: 1,
  beforeFavorite: (obj) => { obj.checked = true; return obj; },
};
"#,
    );

    let manager = start(&directory);
    let token = manager
        .hook_token(PluginHook::BeforeFavorite)
        .expect("availability check should reply")
        .expect("hook should be registered");
    manager
        .unload_plugin("checked.js")
        .expect("unload should succeed");
    let stale_after_unload = manager
        .run_hook_checked(PluginHook::BeforeFavorite, token, &json!({}))
        .expect_err("unload between check and run must fail closed");
    assert!(stale_after_unload.contains("stale plugin hook token"));

    let reloaded = manager
        .reload_plugin("checked.js")
        .expect("reload should succeed");
    assert_eq!(reloaded.plugins[0].state, PluginState::Loaded);
    let current_token = manager
        .hook_token(PluginHook::BeforeFavorite)
        .expect("availability check should reply")
        .expect("reloaded hook should be registered");
    let transformed = manager
        .run_hook_checked(PluginHook::BeforeFavorite, current_token, &json!({}))
        .expect("current token should run the strict chain");
    assert_eq!(transformed["checked"], true);

    manager
        .reload_plugin("checked.js")
        .expect("second reload should succeed");
    let stale_after_reload = manager
        .run_hook_checked(PluginHook::BeforeFavorite, current_token, &json!({}))
        .expect_err("reload between check and run must fail closed");
    assert!(stale_after_reload.contains("stale plugin hook token"));
    manager.shutdown();
}

#[test]
fn resolves_relative_imports_and_a_cycle_back_to_the_cached_root_module() {
    let directory = TestDirectory::new("relative-import-cycle");
    let helper_directory = directory.path().join("lib");
    fs::create_dir(&helper_directory).expect("helper directory should be created");
    fs::write(
        helper_directory.join("helper.js"),
        r#"
import { marker } from "../plugin.js";
export function decorate(obj) {
  obj.imported = marker;
  return obj;
}
"#,
    )
    .expect("helper module should be writable");
    directory.write(
        "plugin.js",
        r#"
import { decorate } from "./lib/helper.js";
export const marker = "relative-cycle";
export default {
  version: 1,
  beforeCreatePost: (obj) => decorate(obj),
};
"#,
    );

    let manager = start(&directory);
    let snapshot = manager.snapshot().expect("cyclic module graph should load");
    assert_eq!(snapshot.plugins.len(), 1);
    assert_eq!(snapshot.plugins[0].state, PluginState::Loaded);
    let transformed = manager
        .run_hook(PluginHook::BeforeCreatePost, &json!({}))
        .expect("imported hook should run");
    assert_eq!(transformed["imported"], "relative-cycle");
    manager.shutdown();
}

#[test]
fn best_effort_hooks_keep_the_last_successful_value_and_log_failures() {
    let directory = TestDirectory::new("best-effort");
    directory.write(
        "00-first.js",
        r#"
export default {
  version: 1,
  afterFavorite: (obj) => { obj.order.push("first"); return obj; },
};
"#,
    );
    directory.write(
        "10-failing.js",
        r#"
export default {
  version: 1,
  afterFavorite: () => { throw new Error("broken after hook"); },
};
"#,
    );
    directory.write(
        "20-last.js",
        r#"
export default {
  version: 1,
  afterFavorite: (obj) => { obj.order.push("last"); return obj; },
};
"#,
    );

    let manager = start(&directory);
    let transformed =
        manager.run_hook_best_effort(PluginHook::AfterFavorite, &json!({"order": []}));
    assert_eq!(transformed["order"], json!(["first", "last"]));
    let snapshot = manager
        .snapshot()
        .expect("snapshot should remain available");
    let failed = snapshot
        .plugins
        .iter()
        .find(|plugin| plugin.id == "10-failing.js")
        .expect("failing plugin should remain discovered");
    assert_eq!(failed.state, PluginState::Loaded);
    assert!(failed
        .logs
        .iter()
        .any(|entry| entry.message.contains("broken after hook")));
    manager.shutdown();
}

#[test]
fn accepts_the_singular_compose_button_compatibility_alias() {
    let directory = TestDirectory::new("button-alias");
    directory.write(
        "alias.js",
        r#"
export default {
  version: 1,
  registerComposeButton: {
    icon: "compat",
    label: "Compatibility",
    onClick: (obj) => obj,
  },
};
"#,
    );

    let manager = start(&directory);
    let snapshot = manager.snapshot().expect("alias should load");
    assert_eq!(snapshot.compose_buttons.len(), 1);
    assert_eq!(snapshot.compose_buttons[0].icon, "compat");
    assert_eq!(
        snapshot.compose_buttons[0].label.as_deref(),
        Some("Compatibility")
    );
    manager.shutdown();
}
