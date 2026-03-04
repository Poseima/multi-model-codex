use codex_app_server_protocol::AuthMode;
use codex_core::auth::AuthDotJson;
use std::ffi::OsStr;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

const AUTH_JSON_FILENAME: &str = "auth.json";
const MULTI_AUTHS_DIRNAME: &str = "multi_auths";
pub(crate) const LAST_AUTH_BACKUP_FILENAME: &str = "_last_auth_backup.json";

#[derive(Debug, Clone)]
pub(crate) struct SwitchAccountCandidate {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) description: Option<String>,
    pub(crate) disabled_reason: Option<String>,
}

pub(crate) fn discover_switch_account_candidates(
    codex_home: &Path,
) -> io::Result<Vec<SwitchAccountCandidate>> {
    let multi_auths_dir = codex_home.join(MULTI_AUTHS_DIRNAME);
    let mut candidates = Vec::new();

    for entry in fs::read_dir(&multi_auths_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !entry.file_type()?.is_file() {
            continue;
        }
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }

        let Some(name_os) = path.file_name() else {
            continue;
        };
        let name = name_os.to_string_lossy().to_string();
        if name == LAST_AUTH_BACKUP_FILENAME {
            continue;
        }

        let (description, disabled_reason) = match read_auth_dot_json(path.as_path()) {
            Ok(auth) => (Some(auth_profile_description(&auth)), None),
            Err(err) => (
                None,
                Some(format!("Invalid auth JSON, cannot switch: {err}")),
            ),
        };

        candidates.push(SwitchAccountCandidate {
            name,
            path,
            description,
            disabled_reason,
        });
    }

    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(candidates)
}

pub(crate) fn switch_account_auth(codex_home: &Path, selected_auth_path: &Path) -> io::Result<()> {
    // Validate before touching the active auth file.
    let selected_contents = fs::read_to_string(selected_auth_path)?;
    let _: AuthDotJson = serde_json::from_str(&selected_contents)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let auth_json_path = codex_home.join(AUTH_JSON_FILENAME);
    let backup_path = codex_home
        .join(MULTI_AUTHS_DIRNAME)
        .join(LAST_AUTH_BACKUP_FILENAME);

    if auth_json_path.is_file() {
        fs::copy(&auth_json_path, &backup_path)?;
    }

    atomic_write_auth_json(codex_home, &auth_json_path, selected_contents.as_bytes())
}

fn read_auth_dot_json(path: &Path) -> io::Result<AuthDotJson> {
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn auth_profile_description(auth: &AuthDotJson) -> String {
    let mode = match auth.auth_mode {
        Some(AuthMode::ApiKey) => "API key",
        Some(AuthMode::Chatgpt) => "ChatGPT",
        Some(AuthMode::ChatgptAuthTokens) => "ChatGPT external tokens",
        None => {
            if auth.openai_api_key.is_some() {
                "API key"
            } else if auth.tokens.is_some() {
                "ChatGPT"
            } else {
                "Unknown auth mode"
            }
        }
    };

    if let Some(tokens) = &auth.tokens {
        if let Some(email) = tokens.id_token.email.as_deref() {
            return format!("{mode} | {email}");
        }
        if let Some(account_id) = tokens
            .account_id
            .as_deref()
            .or(tokens.id_token.chatgpt_account_id.as_deref())
        {
            return format!("{mode} | workspace {account_id}");
        }
    }

    mode.to_string()
}

fn atomic_write_auth_json(
    codex_home: &Path,
    auth_json_path: &Path,
    contents: &[u8],
) -> io::Result<()> {
    let tmp_path = codex_home.join(format!("{AUTH_JSON_FILENAME}.tmp.{}", std::process::id()));

    let mut options = OpenOptions::new();
    options.truncate(true).write(true).create(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    let write_result = (|| -> io::Result<()> {
        let mut file = options.open(&tmp_path)?;
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&tmp_path, auth_json_path)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }

    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn discover_only_lists_json_candidates_sorted_and_skips_backup_file() {
        let codex_home = tempdir().expect("tempdir");
        let multi_auths = codex_home.path().join(MULTI_AUTHS_DIRNAME);
        fs::create_dir_all(&multi_auths).expect("create multi_auths");

        fs::write(multi_auths.join("b.json"), "{\"OPENAI_API_KEY\":\"sk-b\"}").expect("write b");
        fs::write(multi_auths.join("a.json"), "{\"OPENAI_API_KEY\":\"sk-a\"}").expect("write a");
        fs::write(
            multi_auths.join(LAST_AUTH_BACKUP_FILENAME),
            "{\"OPENAI_API_KEY\":\"sk-backup\"}",
        )
        .expect("write backup");
        fs::write(multi_auths.join("ignore.txt"), "ignore").expect("write txt");

        let candidates =
            discover_switch_account_candidates(codex_home.path()).expect("discover candidates");

        let names = candidates
            .iter()
            .map(|candidate| candidate.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a.json".to_string(), "b.json".to_string()]);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.disabled_reason.is_none())
        );
    }

    #[test]
    fn discover_marks_invalid_auth_json_as_disabled() {
        let codex_home = tempdir().expect("tempdir");
        let multi_auths = codex_home.path().join(MULTI_AUTHS_DIRNAME);
        fs::create_dir_all(&multi_auths).expect("create multi_auths");
        fs::write(multi_auths.join("bad.json"), "{invalid").expect("write invalid json");

        let candidates =
            discover_switch_account_candidates(codex_home.path()).expect("discover candidates");

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].disabled_reason.is_some());
    }

    #[test]
    fn switch_account_auth_replaces_auth_json_and_creates_backup() {
        let codex_home = tempdir().expect("tempdir");
        let multi_auths = codex_home.path().join(MULTI_AUTHS_DIRNAME);
        fs::create_dir_all(&multi_auths).expect("create multi_auths");

        let active_auth_path = codex_home.path().join(AUTH_JSON_FILENAME);
        let old_auth = "{\"OPENAI_API_KEY\":\"sk-old\"}";
        fs::write(&active_auth_path, old_auth).expect("write old auth");

        let selected_path = multi_auths.join("profile.json");
        let new_auth = "{\"OPENAI_API_KEY\":\"sk-new\"}";
        fs::write(&selected_path, new_auth).expect("write selected auth");

        switch_account_auth(codex_home.path(), &selected_path).expect("switch auth");

        let current_auth = fs::read_to_string(&active_auth_path).expect("read current auth");
        assert_eq!(current_auth, new_auth);

        let backup = fs::read_to_string(multi_auths.join(LAST_AUTH_BACKUP_FILENAME))
            .expect("read backup auth");
        assert_eq!(backup, old_auth);
    }
}
