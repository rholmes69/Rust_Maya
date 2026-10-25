
# Ollama Optimization Profile for Rust_Maya

- Endpoint: http://127.0.0.1:11434
- Engine: Ollama on Windows
- Rules:
  - Keep models pinned in VRAM using keep_alive: -1.
  - Flash attention must be active (OLLAMA_FLASH_ATTENTION=1).
  - Enforce bounded context windows (num_ctx: 2048 or 4096) to prevent CPU offload.
  - Client connections in Rust must use connection pooling and streaming responses.
