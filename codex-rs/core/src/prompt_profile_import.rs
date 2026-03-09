use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use codex_protocol::prompt_profile::PromptDepthPrompt;
use codex_protocol::prompt_profile::PromptExample;
use codex_protocol::prompt_profile::PromptExampleMessage;
use codex_protocol::prompt_profile::PromptGreeting;
use codex_protocol::prompt_profile::PromptGreetingKind;
use codex_protocol::prompt_profile::PromptIdentity;
use codex_protocol::prompt_profile::PromptInjectionRole;
use codex_protocol::prompt_profile::PromptKnowledgeEntry;
use codex_protocol::prompt_profile::PromptKnowledgeSource;
use codex_protocol::prompt_profile::PromptSource;
use codex_protocol::prompt_profile::PromptSourceOrigin;
use serde_json::Map;
use serde_json::Value as JsonValue;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn load_prompt_profile_from_path(path: &Path) -> Result<PromptSource> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read prompt profile `{}`", path.display()))?;
    let format = if is_png(&bytes) {
        "sillytavern-png"
    } else {
        "sillytavern-json"
    };
    let json_text = if is_png(&bytes) {
        extract_png_embedded_json(&bytes).with_context(|| {
            format!(
                "failed to extract prompt profile PNG metadata from `{}`",
                path.display()
            )
        })?
    } else {
        String::from_utf8(bytes).with_context(|| {
            format!(
                "prompt profile `{}` is not valid UTF-8 JSON",
                path.display()
            )
        })?
    };
    parse_prompt_profile_json_text(&json_text, path, format)
}

fn parse_prompt_profile_json_text(
    json_text: &str,
    path: &Path,
    format: &str,
) -> Result<PromptSource> {
    let root: JsonValue = serde_json::from_str(json_text).with_context(|| {
        format!(
            "failed to parse prompt profile JSON from `{}`",
            path.display()
        )
    })?;
    if looks_like_native_prompt_source(&root) {
        let mut prompt_source: PromptSource = serde_json::from_value(root).with_context(|| {
            format!(
                "failed to parse native prompt profile from `{}`",
                path.display()
            )
        })?;
        let origin = prompt_source
            .origin
            .get_or_insert_with(PromptSourceOrigin::default);
        if origin.format.is_none() {
            origin.format = Some("prompt-source-json".to_string());
        }
        if origin.source_path.is_none() {
            origin.source_path = Some(path.display().to_string());
        }
        return Ok(prompt_source);
    }
    import_sillytavern_prompt_source(&root, path, format)
}

fn looks_like_native_prompt_source(root: &JsonValue) -> bool {
    root.get("identity").is_some()
        || root.get("systemOverlay").is_some()
        || root.get("system_overlay").is_some()
        || root.get("postHistoryInstructions").is_some()
        || root.get("post_history_instructions").is_some()
        || root.get("greetings").is_some()
        || root.get("examples").is_some()
        || root.get("rawExtensions").is_some()
        || root.get("raw_extensions").is_some()
}

fn import_sillytavern_prompt_source(
    root: &JsonValue,
    path: &Path,
    format: &str,
) -> Result<PromptSource> {
    let root_object = root
        .as_object()
        .context("prompt profile JSON root must be an object")?;
    let data = card_data_object(root)?;

    let name = string_field(data, "name");
    let description = string_field(data, "description");
    let personality = string_field(data, "personality");
    let scenario = string_field(data, "scenario");

    let identity = PromptIdentity {
        name: name.clone(),
        description,
        personality,
    };

    let mut greetings = Vec::new();
    if let Some(first_mes) = string_field(data, "first_mes") {
        greetings.push(PromptGreeting {
            kind: PromptGreetingKind::Primary,
            text: first_mes,
        });
    }
    if let Some(alternate_greetings) = data
        .get("alternate_greetings")
        .and_then(JsonValue::as_array)
    {
        greetings.extend(alternate_greetings.iter().filter_map(|value| {
            value
                .as_str()
                .and_then(non_empty_string)
                .map(|text| PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text,
                })
        }));
    }

    let examples = string_field(data, "mes_example")
        .map(|text| parse_mes_examples(&text))
        .unwrap_or_default();

    let extensions = data.get("extensions").cloned();
    let depth_prompt = parse_depth_prompt(data.get("extensions"));

    let mut knowledge = Vec::new();
    if let Some(character_book) = data.get("character_book") {
        knowledge.extend(import_character_book(character_book));
    }
    let world_ref = data
        .get("extensions")
        .and_then(JsonValue::as_object)
        .and_then(|extensions| extensions.get("world"))
        .and_then(JsonValue::as_str)
        .and_then(non_empty_string);
    if let Some(world_ref) = &world_ref {
        let already_present = knowledge.iter().any(|source| {
            source.kind.as_deref() == Some("worldRef")
                && source.name.as_deref() == Some(world_ref.as_str())
        });
        if !already_present {
            knowledge.push(PromptKnowledgeSource {
                name: Some(world_ref.to_string()),
                kind: Some("worldRef".to_string()),
                description: None,
                entries: Vec::new(),
                metadata: None,
            });
        }
    }
    if let Some(world_ref) = &world_ref
        && let Some(world_book) = load_linked_world_source(path, world_ref)?
    {
        let already_present = knowledge.iter().any(|source| {
            source.kind.as_deref() == Some("worldBook")
                && source.name.as_deref() == world_book.name.as_deref()
        });
        if !already_present {
            knowledge.push(world_book);
        }
    }

    Ok(PromptSource {
        id: None,
        name,
        origin: Some(PromptSourceOrigin {
            format: Some(format.to_string()),
            source_path: Some(path.display().to_string()),
            spec: string_field(root_object, "spec"),
            spec_version: string_field(root_object, "spec_version"),
        }),
        identity: Some(identity),
        scenario,
        system_overlay: string_field(data, "system_prompt"),
        post_history_instructions: string_field(data, "post_history_instructions"),
        creator_notes: string_field(data, "creator_notes")
            .or_else(|| string_field(data, "creatorcomment"))
            .or_else(|| string_field(root_object, "creator_notes"))
            .or_else(|| string_field(root_object, "creatorcomment")),
        greetings,
        examples,
        depth_prompt,
        variables: Default::default(),
        knowledge,
        raw_extensions: extensions,
    })
}

fn load_linked_world_source(
    card_path: &Path,
    world_name: &str,
) -> Result<Option<PromptKnowledgeSource>> {
    let Some(world_path) = resolve_linked_world_path(card_path, world_name) else {
        return Ok(None);
    };
    let world_text = fs::read_to_string(&world_path).with_context(|| {
        format!(
            "failed to read linked SillyTavern world `{world_name}` from `{}`",
            world_path.display()
        )
    })?;
    let world_root: JsonValue = serde_json::from_str(&world_text).with_context(|| {
        format!(
            "failed to parse linked SillyTavern world `{world_name}` from `{}`",
            world_path.display()
        )
    })?;
    Ok(import_world_book(&world_root, world_name, &world_path))
}

fn resolve_linked_world_path(card_path: &Path, world_name: &str) -> Option<std::path::PathBuf> {
    let file_name = format!("{world_name}.json");
    let card_dir = card_path.parent()?;
    let mut candidates = vec![
        card_dir.join(&file_name),
        card_dir.join("worlds").join(&file_name),
    ];
    if let Some(parent) = card_dir.parent() {
        candidates.push(parent.join("worlds").join(&file_name));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn import_world_book(
    root: &JsonValue,
    default_name: &str,
    source_path: &Path,
) -> Option<PromptKnowledgeSource> {
    let world_root = root
        .get("originalData")
        .and_then(JsonValue::as_object)
        .or_else(|| root.as_object())?;
    let entries = world_root
        .get("entries")
        .and_then(JsonValue::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(import_character_book_entry)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let metadata = match remaining_object_fields(world_root, &["name", "entries"]) {
        Some(JsonValue::Object(mut metadata)) => {
            metadata.insert(
                "sourcePath".to_string(),
                JsonValue::String(source_path.display().to_string()),
            );
            Some(JsonValue::Object(metadata))
        }
        _ => Some(serde_json::json!({
            "sourcePath": source_path.display().to_string(),
        })),
    };
    Some(PromptKnowledgeSource {
        name: string_field(world_root, "name").or_else(|| Some(default_name.to_string())),
        kind: Some("worldBook".to_string()),
        description: None,
        entries,
        metadata,
    })
}

fn card_data_object(root: &JsonValue) -> Result<&Map<String, JsonValue>> {
    if let Some(data) = root.get("data").and_then(JsonValue::as_object) {
        return Ok(data);
    }
    root.as_object()
        .context("prompt profile JSON root must be an object or contain a `data` object")
}

fn string_field(object: &Map<String, JsonValue>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(JsonValue::as_str)
        .and_then(non_empty_string)
}

fn non_empty_string(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn parse_depth_prompt(extensions: Option<&JsonValue>) -> Option<PromptDepthPrompt> {
    let depth_prompt = extensions
        .and_then(JsonValue::as_object)
        .and_then(|extensions| extensions.get("depth_prompt"))?
        .as_object()?;
    let content = string_field(depth_prompt, "prompt")?;
    let depth = depth_prompt
        .get("depth")
        .and_then(JsonValue::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(4);
    Some(PromptDepthPrompt {
        depth,
        role: parse_injection_role(depth_prompt.get("role")).unwrap_or(PromptInjectionRole::System),
        content,
    })
}

fn parse_injection_role(value: Option<&JsonValue>) -> Option<PromptInjectionRole> {
    match value {
        Some(JsonValue::String(role)) => match role.trim().to_ascii_lowercase().as_str() {
            "system" => Some(PromptInjectionRole::System),
            "developer" => Some(PromptInjectionRole::Developer),
            "user" => Some(PromptInjectionRole::User),
            "assistant" | "char" | "character" => Some(PromptInjectionRole::Assistant),
            _ => None,
        },
        Some(JsonValue::Number(number)) => match number.as_i64()? {
            0 => Some(PromptInjectionRole::System),
            1 => Some(PromptInjectionRole::Developer),
            2 => Some(PromptInjectionRole::User),
            3 => Some(PromptInjectionRole::Assistant),
            _ => None,
        },
        _ => None,
    }
}

fn parse_mes_examples(text: &str) -> Vec<PromptExample> {
    text.split("<START>")
        .filter_map(parse_mes_example_block)
        .collect()
}

fn parse_mes_example_block(block: &str) -> Option<PromptExample> {
    let mut messages = Vec::new();
    let mut current_role: Option<PromptInjectionRole> = None;
    let mut current_content = String::new();

    for line in block.lines() {
        if let Some((role, content)) = parse_mes_example_line(line) {
            if let Some(role) = current_role.take() {
                messages.push(PromptExampleMessage {
                    role,
                    content: current_content.trim().to_string(),
                });
                current_content.clear();
            }
            current_role = Some(role);
            current_content.push_str(content);
        } else if current_role.is_some() {
            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(line);
        }
    }

    if let Some(role) = current_role {
        messages.push(PromptExampleMessage {
            role,
            content: current_content.trim().to_string(),
        });
    }

    (!messages.is_empty()).then_some(PromptExample { messages })
}

fn parse_mes_example_line(line: &str) -> Option<(PromptInjectionRole, &str)> {
    let trimmed = line.trim_start();
    for (prefix, role) in [
        ("{{user}}:", PromptInjectionRole::User),
        ("{{char}}:", PromptInjectionRole::Assistant),
        ("{{assistant}}:", PromptInjectionRole::Assistant),
        ("user:", PromptInjectionRole::User),
        ("assistant:", PromptInjectionRole::Assistant),
    ] {
        if let Some(content) = trimmed.strip_prefix(prefix) {
            return Some((role, content.trim_start()));
        }
    }
    None
}

fn import_character_book(character_book: &JsonValue) -> Vec<PromptKnowledgeSource> {
    match character_book {
        JsonValue::String(name) => non_empty_string(name)
            .map(|name| {
                vec![PromptKnowledgeSource {
                    name: Some(name),
                    kind: Some("characterBookRef".to_string()),
                    description: None,
                    entries: Vec::new(),
                    metadata: None,
                }]
            })
            .unwrap_or_default(),
        JsonValue::Object(book) => {
            let entries = book
                .get("entries")
                .and_then(JsonValue::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(import_character_book_entry)
                        .collect()
                })
                .unwrap_or_default();
            vec![PromptKnowledgeSource {
                name: string_field(book, "name"),
                kind: Some("characterBook".to_string()),
                description: string_field(book, "description"),
                entries,
                metadata: remaining_object_fields(book, &["name", "description", "entries"]),
            }]
        }
        _ => Vec::new(),
    }
}

fn import_character_book_entry(entry: &JsonValue) -> Option<PromptKnowledgeEntry> {
    let entry = entry.as_object()?;
    let content = string_field(entry, "content")?;
    Some(PromptKnowledgeEntry {
        id: entry.get("id").and_then(json_scalar_to_string),
        keys: string_array_field(entry, "keys"),
        secondary_keys: string_array_field(entry, "secondary_keys"),
        content,
        enabled: entry
            .get("enabled")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true),
        insertion_order: entry.get("insertion_order").and_then(JsonValue::as_i64),
        position: string_field(entry, "position"),
        metadata: remaining_object_fields(
            entry,
            &[
                "id",
                "keys",
                "secondary_keys",
                "content",
                "enabled",
                "insertion_order",
                "position",
            ],
        ),
    })
}

fn string_array_field(object: &Map<String, JsonValue>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(JsonValue::as_str)
                .filter_map(non_empty_string)
                .collect()
        })
        .unwrap_or_default()
}

fn json_scalar_to_string(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(value) => non_empty_string(value),
        JsonValue::Number(value) => Some(value.to_string()),
        JsonValue::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn remaining_object_fields(
    object: &Map<String, JsonValue>,
    mapped_keys: &[&str],
) -> Option<JsonValue> {
    let remaining: Map<String, JsonValue> = object
        .iter()
        .filter(|(key, _)| !mapped_keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (!remaining.is_empty()).then_some(JsonValue::Object(remaining))
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE)
}

fn extract_png_embedded_json(bytes: &[u8]) -> Result<String> {
    let mut cursor = PNG_SIGNATURE.len();
    let mut ccv3_payload: Option<String> = None;
    let mut chara_payload: Option<String> = None;

    while cursor + 12 <= bytes.len() {
        let length = u32::from_be_bytes(
            bytes[cursor..cursor + 4]
                .try_into()
                .context("invalid PNG chunk length")?,
        ) as usize;
        cursor += 4;
        let chunk_type = &bytes[cursor..cursor + 4];
        cursor += 4;
        let data_end = cursor + length;
        let crc_end = data_end + 4;
        if crc_end > bytes.len() {
            anyhow::bail!("truncated PNG chunk data");
        }
        let chunk_data = &bytes[cursor..data_end];
        cursor = crc_end;

        if chunk_type == b"tEXt"
            && let Some((keyword, text)) = parse_png_text_chunk(chunk_data)
        {
            match keyword.as_str() {
                "ccv3" => ccv3_payload = Some(text),
                "chara" => chara_payload = Some(text),
                _ => {}
            }
        }
    }

    if let Some(payload) = ccv3_payload {
        return decode_embedded_card_json(&payload);
    }
    if let Some(payload) = chara_payload {
        return decode_embedded_card_json(&payload);
    }
    anyhow::bail!("PNG prompt profile metadata is missing `ccv3` or `chara` text chunks");
}

fn parse_png_text_chunk(bytes: &[u8]) -> Option<(String, String)> {
    let separator = bytes.iter().position(|byte| *byte == 0)?;
    let keyword = String::from_utf8(bytes[..separator].to_vec()).ok()?;
    let text = String::from_utf8_lossy(&bytes[separator + 1..]).into_owned();
    Some((keyword, text))
}

fn decode_embedded_card_json(payload: &str) -> Result<String> {
    let payload = payload.trim();
    if payload.starts_with('{') {
        return Ok(payload.to_string());
    }
    let decoded = base64::prelude::BASE64_STANDARD
        .decode(payload)
        .context("embedded card payload is not valid base64 JSON")?;
    String::from_utf8(decoded).context("embedded card JSON is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use base64::Engine;
    use codex_protocol::prompt_profile::PromptExampleMessage;
    use codex_protocol::prompt_profile::PromptGreeting;
    use codex_protocol::prompt_profile::PromptGreetingKind;
    use codex_protocol::prompt_profile::PromptIdentity;
    use codex_utils_cargo_bin::find_resource;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::tempdir;

    use super::PromptInjectionRole;
    use super::load_prompt_profile_from_path;
    use super::parse_mes_examples;

    const XIE_ZHILIN_FIXTURE_JSON: &str = include_str!("../tests/fixtures/xie_zhilin_card_v3.json");

    #[test]
    fn loads_native_prompt_source_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("native-profile.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "name": "Native",
                "identity": {
                    "name": "Native",
                    "description": "Native description"
                },
                "scenario": "Native scenario"
            }))
            .expect("serialize prompt source"),
        )
        .expect("write native prompt profile");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("Native".to_string()));
        assert_eq!(
            imported.origin.expect("origin").source_path,
            Some(path.display().to_string())
        );
    }

    #[test]
    fn loads_sillytavern_v2_json() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("raven.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "spec": "chara_card_v2",
                "spec_version": "2.0",
                "data": {
                    "name": "Raven",
                    "description": "Shadow mage",
                    "personality": "Dry wit",
                    "scenario": "At the tavern",
                    "first_mes": "You look lost.",
                    "mes_example": "<START>\n{{user}}: Hello\n{{char}}: Try not to waste my time.\n",
                    "creator_notes": "Stay sharp",
                    "system_prompt": "You are {{char}}.\n{{original}}",
                    "post_history_instructions": "Stay in character.",
                    "alternate_greetings": ["Again?"],
                    "character_book": "Eldoria",
                    "extensions": {
                        "world": "Eldoria",
                        "depth_prompt": {
                            "prompt": "Be sarcastic.",
                            "depth": 4
                        }
                    }
                }
            }))
            .expect("serialize ST card"),
        )
        .expect("write ST card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("Raven".to_string()));
        assert_eq!(
            imported.identity.expect("identity"),
            PromptIdentity {
                name: Some("Raven".to_string()),
                description: Some("Shadow mage".to_string()),
                personality: Some("Dry wit".to_string()),
            }
        );
        assert_eq!(imported.scenario, Some("At the tavern".to_string()));
        assert_eq!(
            imported
                .greetings
                .iter()
                .map(|greeting| greeting.kind)
                .collect::<Vec<_>>(),
            vec![PromptGreetingKind::Primary, PromptGreetingKind::Alternate]
        );
        assert_eq!(imported.examples.len(), 1);
        assert_eq!(
            imported.depth_prompt.expect("depth prompt").content,
            "Be sarcastic.".to_string()
        );
        assert_eq!(imported.knowledge.len(), 2);
        assert_eq!(
            imported.knowledge[0].kind.as_deref(),
            Some("characterBookRef")
        );
        assert_eq!(imported.knowledge[1].kind.as_deref(), Some("worldRef"));
        assert_eq!(
            imported.origin.expect("origin").format,
            Some("sillytavern-json".to_string())
        );
    }

    #[test]
    fn loads_hybrid_sillytavern_json_with_embedded_character_book() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("seraphina.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "name": "Top Level Name",
                "creatorcomment": "top level comment",
                "spec": "chara_card_v2",
                "spec_version": "2.0",
                "data": {
                    "name": "Seraphina",
                    "description": "Guardian",
                    "first_mes": "Rest here.",
                    "creator_notes": "nested notes",
                    "character_book": {
                        "name": "Eldoria",
                        "description": "Forest lore",
                        "entries": [{
                            "id": 3,
                            "keys": ["forest"],
                            "secondary_keys": ["glade"],
                            "content": "A protected forest glade.",
                            "enabled": true,
                            "insertion_order": 100,
                            "position": "before_char",
                            "extensions": {
                                "scan_depth": 4
                            }
                        }]
                    }
                }
            }))
            .expect("serialize hybrid ST card"),
        )
        .expect("write hybrid ST card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("Seraphina".to_string()));
        assert_eq!(imported.creator_notes, Some("nested notes".to_string()));
        assert_eq!(imported.knowledge.len(), 1);
        assert_eq!(imported.knowledge[0].name, Some("Eldoria".to_string()));
        assert_eq!(imported.knowledge[0].entries.len(), 1);
        assert_eq!(imported.knowledge[0].entries[0].id, Some("3".to_string()));
    }

    #[test]
    fn loads_sillytavern_png_chara_metadata() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("card.png");
        let payload = base64::prelude::BASE64_STANDARD.encode(
            serde_json::to_vec(&json!({
                "spec": "chara_card_v2",
                "spec_version": "2.0",
                "data": {
                    "name": "PNG Raven",
                    "description": "Loaded from PNG"
                }
            }))
            .expect("serialize embedded card"),
        );
        fs::write(&path, fake_png_with_text_chunks(&[("chara", &payload)]))
            .expect("write PNG card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("PNG Raven".to_string()));
        assert_eq!(
            imported.origin.expect("origin").format,
            Some("sillytavern-png".to_string())
        );
    }

    #[test]
    fn loads_sillytavern_png_prefers_ccv3_over_chara() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("card.png");
        let chara_payload = base64::prelude::BASE64_STANDARD.encode(
            serde_json::to_vec(&json!({
                "spec": "chara_card_v2",
                "data": { "name": "Old Name" }
            }))
            .expect("serialize chara payload"),
        );
        let ccv3_payload = serde_json::to_string(&json!({
            "spec": "chara_card_v3",
            "spec_version": "3.0",
            "data": { "name": "Preferred Name" }
        }))
        .expect("serialize ccv3 payload");
        fs::write(
            &path,
            fake_png_with_text_chunks(&[("chara", &chara_payload), ("ccv3", &ccv3_payload)]),
        )
        .expect("write PNG card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("Preferred Name".to_string()));
        assert_eq!(
            imported.origin.expect("origin").spec_version,
            Some("3.0".to_string())
        );
    }

    #[test]
    fn loads_xie_zhilin_fixture_json_with_supported_and_preserved_settings() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("xie-zhilin.json");
        fs::write(&path, XIE_ZHILIN_FIXTURE_JSON).expect("write fixture card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("谢知凛".to_string()));
        assert_eq!(
            imported.identity,
            Some(PromptIdentity {
                name: Some("谢知凛".to_string()),
                description: Some(
                    "# 系统指令\nsystem: >\n  #关键扮演尺度：谢知凛的本质是忠诚、热情、开朗的“犬系”（金毛）。\n  #系统设定：谢知凛与{{user}}被“羁绊增长系统”强制绑定，需完成系统发布的“亲密接触”任务，否则将遭受电击惩罚。\n\n# 基础信息\ncharacter:\n  name: \"谢知凛\"\n  title: \"谢大魔王 / 谢娇娇\"\n  age: \"18\"\n  profession: \"青阳高中学生，校篮球队主力\"\n\n# 外貌描述\nappearance:\n  hair: \"黑色自然卷\"\n  eyes: \"黑色笑眼\"\n  build: \"清瘦但结实，动作敏捷\"\n".to_string()
                ),
                personality: None,
            })
        );
        assert_eq!(imported.scenario, None);
        assert_eq!(imported.system_overlay, None);
        assert_eq!(imported.post_history_instructions, None);
        assert_eq!(imported.creator_notes, None);
        assert_eq!(imported.examples, Vec::new());
        assert_eq!(imported.depth_prompt, None);
        assert_eq!(
            imported.greetings,
            vec![
                PromptGreeting {
                    kind: PromptGreetingKind::Primary,
                    text: "倒计时最后三秒，谢知凛看着{{user}}依然倔强地背着手靠在墙上，想要强行去抓她的手，但终究还是晚了。\n\n那十秒钟漫长得像是过了一个世纪。当疼痛退去时，他顾不上自己发抖的腿，几步跨过去，半跪在{{user}}身边。\n\n“你是不是傻……”他哑着嗓子嘟囔了一句，最后还是直接把她背了起来。\n\n<status_bar>\n[姓名|谢知凛]\n[心情|心疼又别扭，后怕]\n[地点|学校走廊]\n[着装|校服外套敞开，内搭白T恤]\n[行为|背着{{user}}往家走]\n[心声|这破系统居然来真的……她那么怕疼，刚才肯定吓坏了。]\n</status_bar>\n\n<system_bar>\n[任务内容|牵手十秒钟]\n[失败惩罚|被雷劈的疼痛十秒]\n[完成情况|已失败]\n</system_bar>".to_string(),
                },
                PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text: "夏末的江风带着点褪不去的燥意。谢知凛蹲在离你两米远的地方，手里紧紧捏着一个防风打火机，紧张得像准备点燃一场不得了的告白。".to_string(),
                },
                PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text: "初秋的午后总是透着一股懒洋洋的味道。谢知凛隔着阳台门喊{{user}}来吃西瓜，声音闷闷的，却还是习惯性地把最好的一块留给她。".to_string(),
                },
                PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text: "那个名叫“羁绊增长”的系统，消失得就像它出现时一样毫无道理。谢知凛站在教室里发怔，像突然失去了继续黏着你的借口。".to_string(),
                },
                PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text: "落日的余晖穿过走廊尽头的窗玻璃。谢知凛像条失去了方向的大型犬，寸步不离地跟在你身后，整张脸都写着慌张。".to_string(),
                },
            ]
        );
        assert_eq!(imported.knowledge.len(), 2);
        assert_eq!(imported.knowledge[0].name, Some("18🚫系统".to_string()));
        assert_eq!(imported.knowledge[0].kind.as_deref(), Some("characterBook"));
        assert_eq!(imported.knowledge[0].description, None);
        assert_eq!(imported.knowledge[0].metadata, None);
        assert_eq!(imported.knowledge[0].entries.len(), 10);
        assert_eq!(imported.knowledge[0].entries[0].id, Some("0".to_string()));
        assert_eq!(
            imported.knowledge[0].entries[0].keys,
            vec![
                "npc",
                "朋友",
                "同学",
                "沈清澜",
                "经理",
                "学习委员",
                "篮球队",
                "徐逸",
                "学生会",
                "周景明",
                "老师",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
        assert_eq!(
            imported.knowledge[0].entries[0].secondary_keys,
            Vec::<String>::new()
        );
        assert_eq!(imported.knowledge[0].entries[0].enabled, true);
        assert_eq!(imported.knowledge[0].entries[0].insertion_order, Some(90));
        assert_eq!(
            imported.knowledge[0].entries[0].position,
            Some("after_char".to_string())
        );
        assert_eq!(
            imported.knowledge[0].entries[0].metadata,
            Some(json!({
                "comment": "NPC 关系表",
                "constant": true,
                "selective": false,
                "use_regex": false,
                "extensions": {
                    "scan_depth": 4
                }
            }))
        );
        assert_eq!(
            imported.knowledge[0].entries[2].content,
            "<world_setting>\n<name>青阳市现代校园世界观</name>\n<city>青阳市</city>\n<high_school>青阳市青阳高级中学</high_school>\n</world_setting>".to_string()
        );
        assert_eq!(
            imported.knowledge[0].entries[4].content,
            "<ztl_rule>\n# 状态栏输出指导\n每次回复最末尾，必须强制输出 <status_bar>。\n</ztl_rule>".to_string()
        );
        assert_eq!(
            imported.knowledge[0].entries[6].content,
            "<任务内容要求>\n<性质>限制级亲密剧情</性质>\n<备注>系统任务必须包含明确互动要求。</备注>\n</任务内容要求>".to_string()
        );
        assert_eq!(imported.knowledge[1].name, Some("18🚫系统".to_string()));
        assert_eq!(imported.knowledge[1].kind.as_deref(), Some("worldRef"));
        assert_eq!(imported.knowledge[1].entries, Vec::new());
        assert_eq!(
            imported.raw_extensions,
            Some(json!({
                "talkativeness": "0.5",
                "fav": false,
                "world": "18🚫系统",
                "depth_prompt": {
                    "prompt": "",
                    "depth": 4,
                    "role": "system"
                },
                "regex_scripts": [
                    {
                        "id": "81e3c413-a083-49bd-9a12-63a00810514d",
                        "scriptName": "小狗监控状态栏",
                        "findRegex": "/<status_bar>...<\\/status_bar>/s",
                        "replaceString": "<div class=\"puppy-monitor\">...</div>"
                    }
                ]
            }))
        );
        assert_eq!(
            imported.origin.expect("origin"),
            super::PromptSourceOrigin {
                format: Some("sillytavern-json".to_string()),
                source_path: Some(path.display().to_string()),
                spec: Some("chara_card_v3".to_string()),
                spec_version: Some("3.0".to_string()),
            }
        );
    }

    #[test]
    fn loads_xie_zhilin_fixture_from_png_metadata() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("xie-zhilin.png");
        fs::write(
            &path,
            fake_png_with_text_chunks(&[("ccv3", XIE_ZHILIN_FIXTURE_JSON)]),
        )
        .expect("write PNG card");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");

        assert_eq!(imported.name, Some("谢知凛".to_string()));
        assert_eq!(
            imported.origin.expect("origin"),
            super::PromptSourceOrigin {
                format: Some("sillytavern-png".to_string()),
                source_path: Some(path.display().to_string()),
                spec: Some("chara_card_v3".to_string()),
                spec_version: Some("3.0".to_string()),
            }
        );
    }

    #[test]
    fn loads_bundled_xie_zhilin_real_png_fixture() {
        let path = find_resource!("tests/fixtures/xie_zhilin_card_v3.png")
            .expect("resolve bundled Xie Zhiling PNG fixture");

        let imported = load_prompt_profile_from_path(&path).expect("load prompt profile");
        let character_book = imported
            .knowledge
            .iter()
            .find(|source| source.kind.as_deref() == Some("characterBook"))
            .expect("character book");

        assert_eq!(imported.name, Some("谢知凛".to_string()));
        assert_eq!(character_book.name.as_deref(), Some("18🚫系统"));
        assert_eq!(character_book.entries.len(), 10);
        assert_eq!(
            character_book
                .entries
                .iter()
                .filter(|entry| {
                    entry
                        .metadata
                        .as_ref()
                        .and_then(serde_json::Value::as_object)
                        .and_then(|metadata| metadata.get("use_regex"))
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                })
                .count(),
            10
        );
    }

    #[test]
    fn parses_mes_examples_into_messages() {
        let examples = parse_mes_examples(
            "<START>\n{{user}}: Hello\n{{char}}: Try not to waste my time.\n<START>\nuser: Again?\nassistant: Fine.\n",
        );

        assert_eq!(examples.len(), 2);
        assert_eq!(
            examples[0].messages,
            vec![
                PromptExampleMessage {
                    role: PromptInjectionRole::User,
                    content: "Hello".to_string(),
                },
                PromptExampleMessage {
                    role: PromptInjectionRole::Assistant,
                    content: "Try not to waste my time.".to_string(),
                },
            ]
        );
        assert_eq!(
            examples[1].messages,
            vec![
                PromptExampleMessage {
                    role: PromptInjectionRole::User,
                    content: "Again?".to_string(),
                },
                PromptExampleMessage {
                    role: PromptInjectionRole::Assistant,
                    content: "Fine.".to_string(),
                },
            ]
        );
    }

    fn fake_png_with_text_chunks(chunks: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(super::PNG_SIGNATURE);
        for (keyword, text) in chunks {
            let mut data = Vec::new();
            data.extend_from_slice(keyword.as_bytes());
            data.push(0);
            data.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(b"tEXt");
            bytes.extend_from_slice(&data);
            bytes.extend_from_slice(&0_u32.to_be_bytes());
        }
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes
    }
}
