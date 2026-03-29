use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptInjectionRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptGreetingKind {
    Primary,
    Alternate,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptIdentity {
    pub name: Option<String>,
    pub description: Option<String>,
    pub personality: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptGreeting {
    pub kind: PromptGreetingKind,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptExampleMessage {
    pub role: PromptInjectionRole,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptExample {
    #[serde(default)]
    pub messages: Vec<PromptExampleMessage>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptDepthPrompt {
    pub role: PromptInjectionRole,
    pub depth: u32,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptKnowledgeEntry {
    pub content: String,
    #[serde(default)]
    pub enabled: bool,
    pub id: Option<String>,
    pub insertion_order: Option<i64>,
    #[serde(default)]
    pub keys: Vec<String>,
    pub metadata: Option<JsonValue>,
    pub position: Option<String>,
    #[serde(default)]
    pub secondary_keys: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptKnowledgeSource {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub entries: Vec<PromptKnowledgeEntry>,
    pub metadata: Option<JsonValue>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptSourceOrigin {
    pub format: Option<String>,
    pub source_path: Option<String>,
    pub spec: Option<String>,
    pub spec_version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PromptSource {
    pub id: Option<String>,
    pub name: Option<String>,
    pub creator_notes: Option<String>,
    pub identity: Option<PromptIdentity>,
    pub scenario: Option<String>,
    pub system_overlay: Option<String>,
    pub post_history_instructions: Option<String>,
    pub depth_prompt: Option<PromptDepthPrompt>,
    #[serde(default)]
    pub greetings: Vec<PromptGreeting>,
    #[serde(default)]
    pub examples: Vec<PromptExample>,
    #[serde(default)]
    pub knowledge: Vec<PromptKnowledgeSource>,
    #[serde(default)]
    pub variables: std::collections::HashMap<String, String>,
    pub origin: Option<PromptSourceOrigin>,
    pub raw_extensions: Option<JsonValue>,
}
