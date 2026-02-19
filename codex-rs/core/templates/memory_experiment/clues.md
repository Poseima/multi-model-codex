## Project Memory

You have access to project-specific memories from prior sessions.

### Memory Clues
{{ clues_content }}

### How to Retrieve Memories

If your current task matches any memory clues above, use the `spawn_agent` tool
with `agent_type: "memory_retriever"` and a message describing what context you
need. The agent will research your project memories and return synthesized
findings with source references.

After spawning the memory_retriever, use `wait` with `timeout_ms: 300000` to
collect results. The wait returns as soon as the agent finishes — it will NOT
block for the full duration. While the retriever is working, you may start on
parts of the task that are not covered by the memory clues above.

Only retrieve memories when clues are relevant to your current task.
Do not retrieve memories for trivial or unrelated queries.
