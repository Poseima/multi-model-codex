use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptSource {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub origin: Option<PromptSourceOrigin>,
    #[serde(default)]
    pub identity: Option<PromptIdentity>,
    #[serde(default)]
    pub scenario: Option<String>,
    #[serde(default)]
    pub system_overlay: Option<String>,
    #[serde(default)]
    pub post_history_instructions: Option<String>,
    #[serde(default)]
    pub creator_notes: Option<String>,
    #[serde(default)]
    pub greetings: Vec<PromptGreeting>,
    #[serde(default)]
    pub examples: Vec<PromptExample>,
    #[serde(default)]
    pub depth_prompt: Option<PromptDepthPrompt>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub knowledge: Vec<PromptKnowledgeSource>,
    #[serde(default)]
    pub raw_extensions: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptSourceOrigin {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub spec: Option<String>,
    #[serde(default)]
    pub spec_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptIdentity {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub personality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptGreeting {
    pub kind: PromptGreetingKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum PromptGreetingKind {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptExample {
    #[serde(default)]
    pub messages: Vec<PromptExampleMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptExampleMessage {
    pub role: PromptInjectionRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptDepthPrompt {
    pub depth: u32,
    pub role: PromptInjectionRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptKnowledgeSource {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub entries: Vec<PromptKnowledgeEntry>,
    #[serde(default)]
    pub metadata: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptKnowledgeEntry {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub secondary_keys: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub insertion_order: Option<i64>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub metadata: Option<JsonValue>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum PromptInjectionRole {
    System,
    Developer,
    User,
    Assistant,
}
