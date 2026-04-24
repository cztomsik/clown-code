# Compact Skill

Compress and summarize the current conversation context to reduce token usage. When invoked:

1. **Summarize**: Review the conversation history and produce a concise summary of key points, decisions, and current state.
2. **Preserve intent**: Ensure the summary retains all critical context needed to continue the task.
3. **Replace history**: Replace the full conversation history with the compressed summary in the agent's message buffer.
4. **Notify**: Inform the user that the context has been compacted and provide the token savings estimate.

This is useful when the conversation grows too long and token limits are a concern.
