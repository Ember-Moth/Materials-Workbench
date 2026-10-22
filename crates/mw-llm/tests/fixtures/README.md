# 流式响应测试数据

全部是手写的 SSE 数据，没有录制真实请求。事件名、字段和顺序按各家官方文档的示例写（2026-10-22 核对），内容换成了本项目的场景。ID、签名和加密内容都是假的，只用来验证原样保存和回放。

| 文件 | 内容 | 依据 |
|---|---|---|
| `anthropic/tool_use.sse` | 摘要式 thinking 块（含签名增量）、文本、带参数增量的 `tool_use`、`ping`；`message_start` 带缓存用量 | Claude API Streaming messages："Streaming request with tool use" 和 "Streaming request with thinking" |
| `anthropic/refusal.sse` | 输出一部分文本后被拒绝，`message_delta` 带 `stop_details`（`cyber`） | Refusals and fallback："What a refusal looks like"；Stop reasons："Streaming considerations" |
| `anthropic/error.sse` | 输出中途的 `error` 事件（`overloaded_error`） | Streaming messages："Error events" |
| `anthropic/unknown.sse` | `redacted_thinking`、`server_tool_use`、一个虚构的 `future_block`（带未知增量）、未知事件 `future_event`、文本上的 `citations_delta` | Streaming messages："Other events"（未知事件要容忍）；`future_*` 是虚构的 |
| `anthropic/compaction.sse` | 按需压缩的返回：整块到达的 `compaction` 块、`stop_reason: compaction`、用量在 `iterations` 里 | Compaction on demand："Request a summary"、"Count compaction usage" |
| `openai_chat/tool_calls.sse` | 文本加两个并行工具调用，最后一个块带用量（含缓存命中），以 `[DONE]` 结束 | OpenAI Chat Completions 流式块格式（`choices[].delta.tool_calls[]`、`stream_options.include_usage`） |
| `openai_chat/reasoning.sse` | DeepSeek 风格的 `reasoning_content`、DeepSeek 的缓存用量字段，以及一个虚构的厂商字段 `x_vendor_trace` | DeepSeek 推理模型的流式格式；`x_vendor_trace` 是虚构的 |
| `openai_chat/refusal.sse` | `delta.refusal` 文本 | OpenAI Structured Outputs："Refusals" |
| `openai_chat/length.sse` | Ollama 风格：`finish_reason: length`，没有用量、没有 `[DONE]` | Ollama OpenAI 兼容端点 |
| `openai_chat/error.sse` | 输出中途的 `data: {"error": …}` 块 | vLLM 等兼容服务的写法 |
| `openai_responses/tool_call.sse` | 两段推理摘要、带加密内容的推理条目、带 `phase` 的消息、函数调用参数增量、`response.completed` 带缓存用量 | OpenAI Responses 流式事件（Function calling："Streaming"；Reasoning："Reasoning summaries"、"Preserve reasoning without stored responses"） |
| `openai_responses/refusal.sse` | `response.refusal.delta` 与 `refusal` 内容部分 | Responses 流式事件 |
| `openai_responses/incomplete.sse` | `response.incomplete`，原因 `max_output_tokens` | Responses `incomplete_details` |
| `openai_responses/failed.sse` | 输出一部分后 `response.failed`（`server_error`） | Responses 流式事件 |
| `openai_responses/error.sse` | 流里的 `error` 事件（`rate_limit_exceeded`） | Responses 流式事件 |
