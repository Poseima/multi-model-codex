use anyhow::Context;
use anyhow::Result;
use codex_protocol::prompt_profile::PromptDepthPrompt;
use codex_protocol::prompt_profile::PromptGreeting;
use codex_protocol::prompt_profile::PromptGreetingKind;
use codex_protocol::prompt_profile::PromptIdentity;
use codex_protocol::prompt_profile::PromptInjectionRole;
use codex_protocol::prompt_profile::PromptSource;
use codex_protocol::prompt_profile::PromptSourceOrigin;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum PromptProfileOverride {
    #[default]
    Inherit,
    Clear,
    Set {
        prompt_profile: PromptSource,
        prompt_profile_path: Option<PathBuf>,
    },
}

impl PromptProfileOverride {
    pub fn from_prompt_profile(prompt_profile: PromptSource) -> Self {
        let prompt_profile_path = prompt_profile_path_from_source(&prompt_profile);
        Self::Set {
            prompt_profile,
            prompt_profile_path,
        }
    }

    pub fn from_path(prompt_profile: PromptSource, path: &Path) -> Self {
        Self::Set {
            prompt_profile,
            prompt_profile_path: Some(path.to_path_buf()),
        }
    }
}

pub fn load_prompt_profile_from_path(path: &Path) -> Result<PromptSource> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let prompt_profile = match value.get("spec").and_then(Value::as_str) {
        Some("chara_card_v2") => load_chara_card_v2(path, &value),
        _ => serde_json::from_value::<PromptSource>(value)
            .with_context(|| format!("failed to decode prompt profile from {}", path.display())),
    }?;
    ensure_prompt_profile_has_content(&prompt_profile, path)?;
    Ok(prompt_profile)
}

fn load_chara_card_v2(path: &Path, value: &Value) -> Result<PromptSource> {
    let data = value
        .get("data")
        .and_then(Value::as_object)
        .context("missing chara_card_v2 data object")?;

    let name = data.get("name").and_then(Value::as_str).map(str::to_string);
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let personality = data
        .get("personality")
        .and_then(Value::as_str)
        .map(str::to_string);
    let scenario = data
        .get("scenario")
        .and_then(Value::as_str)
        .map(str::to_string);
    let greetings = data
        .get("first_mes")
        .and_then(Value::as_str)
        .map(|text| {
            vec![PromptGreeting {
                kind: PromptGreetingKind::Primary,
                text: text.to_string(),
            }]
        })
        .unwrap_or_default();
    let depth_prompt = data
        .get("extensions")
        .and_then(|extensions| extensions.get("depth_prompt"))
        .and_then(Value::as_object)
        .and_then(|depth_prompt| {
            let content = depth_prompt.get("prompt")?.as_str()?.to_string();
            let depth = depth_prompt
                .get("depth")
                .and_then(Value::as_u64)
                .and_then(|depth| u32::try_from(depth).ok())
                .unwrap_or(4);
            Some(PromptDepthPrompt {
                role: PromptInjectionRole::System,
                depth,
                content,
            })
        });

    Ok(PromptSource {
        name: name.clone(),
        identity: Some(PromptIdentity {
            name,
            description,
            personality,
        }),
        scenario,
        greetings,
        depth_prompt,
        origin: Some(PromptSourceOrigin {
            format: Some("chara_card_v2".to_string()),
            source_path: Some(path.display().to_string()),
            spec: value
                .get("spec")
                .and_then(Value::as_str)
                .map(str::to_string),
            spec_version: value
                .get("spec_version")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        raw_extensions: data.get("extensions").cloned(),
        ..PromptSource::default()
    })
}

pub fn prompt_profile_path_from_source(prompt_profile: &PromptSource) -> Option<PathBuf> {
    prompt_profile
        .origin
        .as_ref()
        .and_then(|origin| origin.source_path.as_deref())
        .map(PathBuf::from)
}

fn ensure_prompt_profile_has_content(prompt_profile: &PromptSource, path: &Path) -> Result<()> {
    let has_content = prompt_profile.id.is_some()
        || prompt_profile.name.is_some()
        || prompt_profile.creator_notes.is_some()
        || prompt_profile.identity.is_some()
        || prompt_profile.scenario.is_some()
        || prompt_profile.system_overlay.is_some()
        || prompt_profile.post_history_instructions.is_some()
        || prompt_profile.depth_prompt.is_some()
        || !prompt_profile.greetings.is_empty()
        || !prompt_profile.examples.is_empty()
        || !prompt_profile.knowledge.is_empty()
        || !prompt_profile.variables.is_empty()
        || prompt_profile.origin.is_some()
        || prompt_profile.raw_extensions.is_some();
    anyhow::ensure!(
        has_content,
        "failed to decode prompt profile from {}: no supported prompt profile fields were found",
        path.display()
    );
    Ok(())
}
