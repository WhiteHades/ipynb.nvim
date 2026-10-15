use super::local::{
    ConnectionInfo, LocalConfig, free_port, launch_local, read_connection_file, read_kernel_spec,
    write_connection_file,
};
use super::session::SessionState;
use super::wire::{MESSAGE_DELIMITER, endpoint, message_value, receive_message, sign_parts};
use super::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn hmac_signature_is_lowercase_sha256_over_four_json_parts() {
    let parts = [
        b"one".to_vec(),
        b"two".to_vec(),
        b"three".to_vec(),
        b"four".to_vec(),
    ];
    let actual = sign_parts(b"secret", &parts);
    assert_eq!(
        actual,
        "4eb80409f5dc6fbe64a10e9da2bbb19a25d5b4206986f2f2a1bb76e12d182180"
    );
    assert_eq!(actual.len(), 64);
    assert!(
        actual
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}

#[test]
fn received_multipart_message_verifies_and_keeps_jupyter_fields() {
    let connection = ConnectionInfo {
        shell_port: 1,
        iopub_port: 2,
        stdin_port: 3,
        control_port: 4,
        ip: "127.0.0.1".to_string(),
        key: b"secret".to_vec(),
        transport: "tcp".to_string(),
        signature_scheme: "hmac-sha256".to_string(),
    };
    let header = json!({"msg_type":"execute_result","msg_id":"abc"});
    let parent = json!({"msg_id":"parent"});
    let metadata = json!({});
    let content = json!({"execution_count":1,"data":{"text/plain":"1"}});
    let parts = [
        serde_json::to_vec(&header).unwrap(),
        serde_json::to_vec(&parent).unwrap(),
        serde_json::to_vec(&metadata).unwrap(),
        serde_json::to_vec(&content).unwrap(),
    ];
    let mut frames = vec![
        MESSAGE_DELIMITER.to_vec(),
        sign_parts(&connection.key, &parts).into_bytes(),
    ];
    frames.extend(parts);
    frames.push(vec![0, 255]);
    let value = receive_message(&frames, &connection, "iopub").unwrap();
    assert_eq!(value["channel"], "iopub");
    assert_eq!(value["msg_type"], "execute_result");
    assert_eq!(value["content"]["data"]["text/plain"], "1");
    assert_eq!(value["buffers"][0], "AP8=");

    frames[1][0] = if frames[1][0] == b'0' { b'1' } else { b'0' };
    assert!(receive_message(&frames, &connection, "iopub").is_err());
}

#[test]
fn connection_file_roundtrip_preserves_ports_and_key() {
    let path = std::env::temp_dir().join(format!("ipynb-kernel-test-{}.json", Uuid::new_v4()));
    let connection = ConnectionInfo {
        shell_port: 1001,
        iopub_port: 1002,
        stdin_port: 1003,
        control_port: 1004,
        ip: "127.0.0.1".to_string(),
        key: b"key".to_vec(),
        transport: "tcp".to_string(),
        signature_scheme: "hmac-sha256".to_string(),
    };
    write_connection_file(&path, &connection, "python3").unwrap();
    let loaded = read_connection_file(&path).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(loaded.shell_port, 1001);
    assert_eq!(loaded.iopub_port, 1002);
    assert_eq!(loaded.stdin_port, 1003);
    assert_eq!(loaded.control_port, 1004);
    assert_eq!(loaded.key, b"key");
}

#[test]
fn local_launch_preserves_kernelspec_argv_and_environment() {
    let directory = tempdir().unwrap();
    let script = directory.path().join("record.sh");
    fs::write(
        &script,
        "#!/bin/sh\noutput=\"$3\"\n{ printf '%s\\n' \"$#\" \"$1\" \"$2\" \"$IPYNB_TEST_ENV\" \"$PWD\" \"$VIRTUAL_ENV\" \"$PATH\"; } > \"$output\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();

    let spec_path = directory.path().join("kernel.json");
    fs::write(
        &spec_path,
        serde_json::to_vec(&json!({
            "argv": [
                "/bin/sh",
                script,
                "{connection_file}",
                "--connection={connection_file}",
                "{resource_dir}/observed"
            ],
            "env": {"IPYNB_TEST_ENV": "${HOME}"},
            "language": "python"
        }))
        .unwrap(),
    )
    .unwrap();

    let mut config = LocalConfig {
        spec: read_kernel_spec(&spec_path, "test").unwrap(),
        // An absolute interpreter path belongs to the kernelspec and must
        // not be replaced by the selected Python executable.
        python: Some("/bin/false".to_string()),
        numpy_legacy_repr: false,
        connection_file: directory.path().join("connection.json"),
        cwd: Some(directory.path().to_owned()),
    };
    let (_connection, mut child) = launch_local(&config).unwrap();
    assert!(child.wait().unwrap().success());

    let observed = fs::read_to_string(directory.path().join("observed")).unwrap();
    let mut lines = observed.lines();
    assert_eq!(lines.next(), Some("3"));
    assert_eq!(lines.next(), Some(config.connection_file.to_str().unwrap()));
    let connection_argument = format!("--connection={}", config.connection_file.display());
    assert_eq!(lines.next(), Some(connection_argument.as_str()));
    let expected_env = std::env::var("HOME").ok();
    assert_eq!(lines.next(), expected_env.as_deref());
    let physical_cwd = directory.path().canonicalize().unwrap();
    assert_eq!(lines.next(), physical_cwd.to_str());
    let expected_venv = std::env::var("VIRTUAL_ENV").unwrap_or_default();
    assert_eq!(lines.next(), Some(expected_venv.as_str()));
    let expected_path = std::env::var("PATH").unwrap_or_default();
    assert_eq!(lines.next(), Some(expected_path.as_str()));
    assert_eq!(lines.next(), None);

    let venv = directory.path().join("venv");
    let bin = venv.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(venv.join("pyvenv.cfg"), "").unwrap();
    let python = bin.join("python");
    std::os::unix::fs::symlink("/bin/sh", &python).unwrap();
    config.spec.argv[0] = python.to_str().unwrap().to_owned();
    for explicit in [false, true] {
        if explicit {
            config.spec.env.extend([
                ("VIRTUAL_ENV".into(), "/custom/venv".into()),
                ("PATH".into(), "/custom/bin".into()),
            ]);
        }
        let (_connection, mut child) = launch_local(&config).unwrap();
        assert!(child.wait().unwrap().success());
        let observed = fs::read_to_string(directory.path().join("observed")).unwrap();
        let lines: Vec<_> = observed.lines().collect();
        assert_eq!(lines[4], physical_cwd.to_str().unwrap());
        assert_eq!(
            lines[5],
            if explicit {
                "/custom/venv"
            } else {
                venv.to_str().unwrap()
            }
        );
        if explicit {
            assert_eq!(lines[6], "/custom/bin");
        } else {
            let paths: Vec<_> = std::env::split_paths(lines[6]).collect();
            assert_eq!(paths[0], bin);
            let inherited: Vec<_> = std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                .collect();
            assert_eq!(paths[1..], inherited);
        }
    }
    let _ = fs::remove_file(&config.connection_file);
}

#[test]
fn pending_execution_is_fifo_and_only_flushes_after_ready() {
    let mut session = SessionState::new();
    let started = Instant::now();
    session.probe_sent(started);
    session
        .pending
        .push_back(("one".to_string(), "1".to_string()));
    session
        .pending
        .push_back(("two".to_string(), "2".to_string()));
    let info = message_value(
        "shell",
        json!({"msg_type":"kernel_info_reply"}),
        json!({"msg_id":"probe", "session":session.session_id}),
        json!({}),
        json!({}),
    );
    let idle = message_value(
        "iopub",
        json!({"msg_type":"status"}),
        json!({"msg_id":"probe", "session":session.session_id}),
        json!({}),
        json!({"execution_state":"idle"}),
    );
    session.observe(&info, "shell", started);
    assert!(!session.maybe_ready(started + Duration::from_millis(200)));
    session.observe(&idle, "iopub", started + Duration::from_millis(1));
    assert!(!session.maybe_ready(started + Duration::from_millis(199)));
    assert!(session.maybe_ready(started + Duration::from_millis(201)));
    assert!(session.ready);
    assert_eq!(session.pending.pop_front().unwrap().0, "one");
    assert_eq!(session.pending.pop_front().unwrap().0, "two");

    let previous_session = session.session_id.clone();
    session.reset();
    assert_ne!(session.session_id, previous_session);
    session.probe_sent(started);
    session.observe(&info, "shell", started);
    session.observe(&idle, "iopub", started);
    assert!(!session.maybe_ready(started + Duration::from_secs(2)));
    let mut current_info = info.clone();
    current_info["parent_header"]["session"] = json!(session.session_id);
    session.observe(&current_info, "shell", started);
    assert!(
        !session.maybe_ready(started + Duration::from_millis(200)),
        "buffered IOPub from a previous session must not release queued work"
    );

    for late_channel in ["shell", "iopub"] {
        session.reset();
        session.probe_sent(started);
        let mut current_info = info.clone();
        current_info["parent_header"]["session"] = json!(session.session_id);
        let mut current_idle = idle.clone();
        current_idle["parent_header"]["session"] = json!(session.session_id);
        let (early, early_channel, late, retry_ms) = if late_channel == "shell" {
            (&current_idle, "iopub", &current_info, 1001)
        } else {
            (&current_info, "shell", &current_idle, 201)
        };
        session.observe(early, early_channel, started);
        let retry = started + Duration::from_millis(retry_ms);
        assert!(session.probe_due(retry));
        session.probe_sent(retry);
        session.observe(late, late_channel, retry + Duration::from_millis(25));
        assert!(!session.probe_due(retry + Duration::from_secs(2)));
        assert!(!session.maybe_ready(retry + Duration::from_millis(224)));
        session.observe(&idle, "iopub", retry + Duration::from_millis(150));
        assert!(!session.maybe_ready(retry + Duration::from_millis(349)));
        assert!(session.maybe_ready(retry + Duration::from_millis(350)));
    }
}

#[test]
fn attached_kernel_executes_queued_code_over_signed_zmq() {
    let connection = ConnectionInfo {
        shell_port: free_port().unwrap(),
        iopub_port: free_port().unwrap(),
        stdin_port: free_port().unwrap(),
        control_port: free_port().unwrap(),
        ip: "127.0.0.1".to_string(),
        key: b"test-key".to_vec(),
        transport: "tcp".to_string(),
        signature_scheme: "hmac-sha256".to_string(),
    };
    let path = std::env::temp_dir().join(format!("ipynb-kernel-test-{}.json", Uuid::new_v4()));
    write_connection_file(&path, &connection, "python3").unwrap();

    let context = zmq::Context::new();
    let shell = context.socket(zmq::ROUTER).unwrap();
    let iopub = context.socket(zmq::PUB).unwrap();
    let control = context.socket(zmq::ROUTER).unwrap();
    shell
        .bind(&endpoint(&connection, connection.shell_port))
        .unwrap();
    iopub
        .bind(&endpoint(&connection, connection.iopub_port))
        .unwrap();
    control
        .bind(&endpoint(&connection, connection.control_port))
        .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let key = connection.key.clone();
    let server = thread::spawn(move || {
        let mut first_probe = true;
        let mut poll_items = [
            shell.as_poll_item(zmq::POLLIN),
            control.as_poll_item(zmq::POLLIN),
        ];
        while !server_stop.load(Ordering::SeqCst) {
            if zmq::poll(&mut poll_items, 20).is_err() {
                break;
            }
            if poll_items[0].is_readable() {
                let frames = shell.recv_multipart(0).unwrap();
                let delimiter = frames
                    .iter()
                    .position(|frame| frame.as_slice() == MESSAGE_DELIMITER)
                    .unwrap();
                let identity = frames[delimiter - 1].clone();
                let header: Value = serde_json::from_slice(&frames[delimiter + 2]).unwrap();
                let content: Value = serde_json::from_slice(&frames[delimiter + 5]).unwrap();
                let msg_type = header["msg_type"].as_str().unwrap();
                if msg_type == "kernel_info_request" {
                    let reply = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "kernel_info_reply",
                        }),
                        header.clone(),
                        json!({}),
                        json!({}),
                    );
                    let mut routed = vec![identity.clone()];
                    routed.extend(reply);
                    shell.send_multipart(routed, 0).unwrap();
                    if first_probe {
                        first_probe = false;
                        continue; // Simulate IOPub's initial slow-joiner loss.
                    }
                    thread::sleep(Duration::from_millis(225));
                    let status = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "status",
                        }),
                        header,
                        json!({}),
                        json!({"execution_state":"idle"}),
                    );
                    iopub.send_multipart(status, 0).unwrap();
                } else if msg_type == "execute_request" {
                    let busy = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "status",
                        }),
                        header.clone(),
                        json!({}),
                        json!({"execution_state":"busy"}),
                    );
                    iopub.send_multipart(busy, 0).unwrap();
                    let result = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "execute_result",
                        }),
                        header.clone(),
                        json!({}),
                        json!({
                            "execution_count": 1,
                            "data": {"text/plain": content["code"]},
                            "metadata": {},
                        }),
                    );
                    iopub.send_multipart(result, 0).unwrap();
                    let reply = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "execute_reply",
                        }),
                        header.clone(),
                        json!({}),
                        json!({"status":"ok","execution_count":1}),
                    );
                    let mut routed = vec![identity];
                    routed.extend(reply);
                    shell.send_multipart(routed, 0).unwrap();
                    let idle = wire_frames(
                        &key,
                        json!({
                            "msg_id": Uuid::new_v4().to_string(),
                            "session": header["session"],
                            "msg_type": "status",
                        }),
                        header,
                        json!({}),
                        json!({"execution_state":"idle"}),
                    );
                    iopub.send_multipart(idle, 0).unwrap();
                }
            }
        }
    });

    let kernel = Kernel::start(path.to_str().unwrap(), None, false, None, None).unwrap();
    let request = kernel.execute("2 + 2").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut messages = Vec::new();
    while Instant::now() < deadline {
        messages.extend(kernel.drain());
        if messages.iter().any(|message| {
            message["header"]["msg_type"] == "execute_result"
                && message["parent_header"]["msg_id"] == request
        }) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let executed = messages.iter().any(|message| {
        message["header"]["msg_type"] == "execute_result"
            && message["parent_header"]["msg_id"] == request
            && message["content"]["data"]["text/plain"] == "2 + 2"
    });
    kernel.shutdown().unwrap();
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    fs::remove_file(path).unwrap();
    assert!(
        executed,
        "delayed probe traffic must not starve queued execution"
    );
}

#[test]
fn stopped_workers_report_once_reject_execution_and_still_join() {
    let directory = tempdir().unwrap();
    let spec = directory.path().join("kernel.json");
    fs::write(
        &spec,
        json!({"argv":["/usr/bin/true"],"language":"r"}).to_string(),
    )
    .unwrap();
    let kernel = Kernel::start("test", None, false, Some(&spec), None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut messages = Vec::new();
    while Instant::now() < deadline {
        messages.extend(kernel.drain());
        if messages
            .iter()
            .any(|message| message["msg_type"] == "kernel_stopped")
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    kernel.join_worker().unwrap();
    messages.extend(kernel.drain());
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["msg_type"] == "kernel_stopped")
            .count(),
        1
    );
    assert!(
        messages
            .iter()
            .all(|message| message["msg_type"] != "transport_error")
    );
    assert!(kernel.execute("do not run").is_err());
    assert!(kernel.restart().is_err());
    kernel.shutdown().unwrap();
    let (commands, receiver) = mpsc::channel();
    let (events, messages) = mpsc::channel();
    drop(receiver);
    drop(events);
    let kernel = Kernel {
        commands,
        wake: None,
        events: messages,
        worker: Mutex::new(None),
        closed: AtomicBool::new(false),
        stopped_reported: AtomicBool::new(false),
        restartable: true,
        language: None,
    };
    let messages = kernel.drain();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msg_type"], "kernel_stopped");
    assert!(kernel.drain().is_empty());
    assert!(kernel.execute("do not run").is_err());
    kernel.shutdown().unwrap();
}

fn wire_frames(
    key: &[u8],
    header: Value,
    parent_header: Value,
    metadata: Value,
    content: Value,
) -> Vec<Vec<u8>> {
    let parts = [
        serde_json::to_vec(&header).unwrap(),
        serde_json::to_vec(&parent_header).unwrap(),
        serde_json::to_vec(&metadata).unwrap(),
        serde_json::to_vec(&content).unwrap(),
    ];
    let mut frames = vec![
        MESSAGE_DELIMITER.to_vec(),
        sign_parts(key, &parts).into_bytes(),
    ];
    frames.extend(parts);
    frames
}
