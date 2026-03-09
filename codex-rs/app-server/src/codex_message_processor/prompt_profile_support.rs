use codex_protocol::prompt_profile::PromptSource;
use std::path::PathBuf;

pub(super) fn resolve_prompt_profile_override(
    prompt_profile: Option<PromptSource>,
    prompt_profile_path: Option<PathBuf>,
) -> std::result::Result<Option<PromptSource>, String> {
    if prompt_profile.is_some() {
        return Ok(prompt_profile);
    }
    let Some(prompt_profile_path) = prompt_profile_path else {
        return Ok(None);
    };
    codex_core::load_prompt_profile_from_path(prompt_profile_path.as_path())
        .map(Some)
        .map_err(|err| {
            format!(
                "failed to load prompt profile `{}`: {err}",
                prompt_profile_path.display()
            )
        })
}
