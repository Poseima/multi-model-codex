//! Exercises a real `responses-api-proxy` process with request dumping enabled, then verifies that
//! parent Responses API requests carry the expected window headers in the dumped exchange.

use anyhow::Result;
use anyhow::anyhow;
use codex_features::Feature;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::Path;
use std::process::Child;
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;

const PARENT_PROMPT: &str = "say done through the proxy";
const PROXY_START_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 5);
const PROXY_POLL_INTERVAL: Duration = Duration::from_millis(/*millis*/ 20);

struct ResponsesApiProxy {
    child: Child,
    port: u16,
}

impl ResponsesApiProxy {
    fn start(upstream_url: &str, dump_dir: &Path) -> Result<Self> {
        let server_info = dump_dir.join("server-info.json");
        let auth_header = dump_dir.join("proxy-auth-header.txt");
        std::fs::write(&auth_header, "dummy\n")?;
        let mut child = StdCommand::new(codex_utils_cargo_bin::cargo_bin("codex")?)
            .args(["responses-api-proxy", "--server-info"])
            .arg(&server_info)
            .args(["--upstream-url", upstream_url, "--dump-dir"])
            .arg(dump_dir)
            .stdin(Stdio::from(std::fs::File::open(&auth_header)?))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let deadline = Instant::now() + PROXY_START_TIMEOUT;
        loop {
            if let Ok(info) = std::fs::read_to_string(&server_info) {
                let port = serde_json::from_str::<Value>(&info)?
                    .get("port")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("proxy server info missing port"))?;
                return Ok(Self {
                    child,
                    port: u16::try_from(port)?,
                });
            }
            if let Some(status) = child.try_wait()? {
                return Err(anyhow!(
                    "responses-api-proxy exited before writing server info: {status}"
                ));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("timed out waiting for responses-api-proxy"));
            }
            std::thread::sleep(PROXY_POLL_INTERVAL);
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

impl Drop for ResponsesApiProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_api_proxy_dumps_parent_and_subagent_identity_headers() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let dump_dir = TempDir::new()?;
    let proxy =
        ResponsesApiProxy::start(&format!("{}/v1/responses", server.uri()), dump_dir.path())?;

    mount_sse_once_match(
        &server,
        |req: &wiremock::Request| request_body_contains(req, PARENT_PROMPT),
        sse(vec![
            ev_response_created("resp-parent-1"),
            ev_assistant_message("msg-parent-1", "done"),
            ev_completed("resp-parent-1"),
        ]),
    )
    .await;

    let proxy_base_url = proxy.base_url();
    let mut builder = test_codex().with_config(move |config| {
        config.model_provider.base_url = Some(proxy_base_url);
        config
            .features
            .disable(Feature::EnableRequestCompression)
            .expect("test config should allow feature update");
    });
    let test = builder.build(&server).await?;
    test.submit_turn(PARENT_PROMPT).await?;

    let dumps = wait_for_proxy_request_dumps(dump_dir.path())?;
    let parent = dumps
        .iter()
        .find(|dump| dump_body_contains(dump, PARENT_PROMPT))
        .ok_or_else(|| anyhow!("missing parent request dump"))?;

    let parent_window_id = header(parent, "x-codex-window-id")
        .ok_or_else(|| anyhow!("parent request missing x-codex-window-id"))?;
    let (_parent_thread_id, parent_generation) = split_window_id(parent_window_id)?;

    assert_eq!(parent_generation, 0);
    assert_eq!(header(parent, "x-openai-subagent"), None);
    assert_eq!(header(parent, "x-codex-parent-thread-id"), None);

    Ok(())
}

fn request_body_contains(req: &wiremock::Request, text: &str) -> bool {
    std::str::from_utf8(&req.body).is_ok_and(|body| body.contains(text))
}

fn wait_for_proxy_request_dumps(dump_dir: &Path) -> Result<Vec<Value>> {
    let deadline = Instant::now() + Duration::from_secs(/*secs*/ 2);
    loop {
        let dumps = read_proxy_request_dumps(dump_dir).unwrap_or_default();
        if dumps
            .iter()
            .any(|dump| dump_body_contains(dump, PARENT_PROMPT))
        {
            return Ok(dumps);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for proxy request dumps, got {}",
                dumps.len()
            ));
        }
        std::thread::sleep(PROXY_POLL_INTERVAL);
    }
}

fn read_proxy_request_dumps(dump_dir: &Path) -> Result<Vec<Value>> {
    let mut dumps = Vec::new();
    for entry in std::fs::read_dir(dump_dir)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("-request.json"))
        {
            dumps.push(serde_json::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(dumps)
}

fn dump_body_contains(dump: &Value, text: &str) -> bool {
    dump.get("body")
        .is_some_and(|body| body.to_string().contains(text))
}

fn header<'a>(dump: &'a Value, name: &str) -> Option<&'a str> {
    dump.get("headers")?.as_array()?.iter().find_map(|header| {
        (header.get("name")?.as_str()?.eq_ignore_ascii_case(name))
            .then(|| header.get("value")?.as_str())
            .flatten()
    })
}

fn split_window_id(window_id: &str) -> Result<(&str, u64)> {
    let (thread_id, generation) = window_id
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("invalid window id header: {window_id}"))?;
    Ok((thread_id, generation.parse::<u64>()?))
}
