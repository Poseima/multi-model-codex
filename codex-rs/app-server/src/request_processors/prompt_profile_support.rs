use std::path::PathBuf;

use codex_core::PromptProfileOverride;
use codex_core::load_prompt_profile_from_path;

pub(super) fn resolve_prompt_profile_override(
    prompt_profile: Option<codex_protocol::prompt_profile::PromptSource>,
    prompt_profile_path: Option<PathBuf>,
) -> Result<PromptProfileOverride, String> {
    match (prompt_profile, prompt_profile_path) {
        (Some(_), Some(_)) => {
            Err("promptProfile cannot be combined with promptProfilePath".to_string())
        }
        (Some(prompt_profile), None) => {
            Ok(PromptProfileOverride::from_prompt_profile(prompt_profile))
        }
        (None, Some(path)) => load_prompt_profile_from_path(path.as_path())
            .map(|prompt_profile| PromptProfileOverride::from_path(prompt_profile, path.as_path()))
            .map_err(|err| {
                format!(
                    "failed to load prompt profile from {}: {err}",
                    path.display()
                )
            }),
        (None, None) => Ok(PromptProfileOverride::Inherit),
    }
}
