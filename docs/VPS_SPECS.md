# Running Rust Maya and Ollama on a VPS

Rust Maya is a desktop app with a 3D viewport: on a VPS it needs a remote desktop and, ideally, a GPU.
Ollama is the heavier part. The setup that usually makes the most sense is to **run the app on your own PC and
put only Ollama on the VPS**: the app talks to Ollama over HTTP, and the Ollama host is a setting.

## Option A — VPS runs only Ollama (recommended)

Size the VPS by the models you want to run:

| Models | RAM / VRAM | Examples |
|---|---|---|
| 3–4B (fast router / small helper) | 4 GB | `qwen2.5:3b`, `llama3.2:3b` |
| 7–8B (good MEL writer) | 8 GB | `qwen2.5-coder:7b`, `llama3.1:8b` |
| 7B vision (Render Check images) | 8–10 GB | `qwen2.5vl:7b`, `llava:7b` |
| 14B (noticeably smarter) | 12–16 GB | `qwen2.5-coder:14b` |
| 32B (best local quality) | 24 GB+ | `qwen2.5-coder:32b` |

- **CPU-only VPS:** 8 vCPU, 16 GB RAM and 50 GB disk runs 7–8B models at roughly 5–10 tokens/second, which is fine
  for short MEL replies.
- **GPU VPS:** one 16–24 GB GPU (e.g. RTX 4000 Ada, L4, A10, RTX 4090), 32 GB RAM and 100 GB disk runs 7–14B models
  fast, plus a vision model. Choose a 24 GB GPU if you want 32B.
- **Security:** Ollama has no login of its own. Don't expose port 11434 publicly; reach it over an SSH tunnel,
  Tailscale or WireGuard, or put a reverse proxy with authentication in front.

Example SSH tunnel from your PC (then set the Ollama host in Rust Maya to `http://127.0.0.1:11434`):

```
ssh -N -L 11434:127.0.0.1:11434 user@your-vps
```

## Option B — the whole app on a VPS (remote desktop)

| | Minimum | Comfortable |
|---|---|---|
| CPU | 4 vCPU | 8+ vCPU (the path tracer and build use every core) |
| RAM | 8 GB | 16 GB, or 32 GB with Ollama on the same box |
| GPU | None (software rendering; workable but laggy) | Any GPU with Vulkan, e.g. an NVIDIA T4 / L4 / A10 |
| Disk | 30 GB | 60 GB+ (models need extra) |
| OS | Ubuntu 22.04 / 24.04 | Same, with desktop packages |
| Remote access | Desktop + VNC | Desktop + a GPU-friendly remote desktop such as NICE DCV, Parsec or Sunshine/Moonlight; plain VNC/RDP feels sluggish for a 3D viewport |

## Building

- **Rust 1.96 or newer**, plus the X11/Wayland development packages and ALSA (for the voice feature) on Linux.
- **About 15 GB of disk** for the `target` build folder, plus about 0.7 GB of downloaded dependencies.
  The finished app is a single ~38 MB file.
- **8 GB RAM** is enough to build; a from-scratch release build takes several minutes on 2 cores.

## Other components

- **Whisper voice** runs on the CPU and needs another 1–2 GB of RAM. On a VPS it's only useful if your
  remote-desktop tool forwards your microphone.
- **OpenRouter** needs no local hardware. If you use it instead of Ollama, a CPU-only VPS with 4 vCPU and 8 GB RAM is
  plenty for the app itself.

## Recommendation

Keep the app on your PC. If your PC can't run the models you want, rent a GPU VPS with about 24 GB of VRAM for Ollama
only and connect to it over Tailscale or an SSH tunnel.

## Reference: the machine used for development testing

| | |
|---|---|
| OS | Ubuntu 24.04.5 LTS, Linux 6.18 (x86_64) |
| CPU | Intel Xeon @ 2.10 GHz, 2 cores (AVX2, AVX-512) |
| RAM | 7.8 GB, no swap |
| GPU | None — viewport rendered in software (Mesa CPU renderer via wgpu's OpenGL backend, `WGPU_BACKEND=gl`) under a 1600×960 virtual display (Xvfb) |
| Rust | 1.97.0 |
| AI | No real models; the AI features were tested against a mock Ollama/OpenRouter server (`tools/mock_ai.py`) |

The million-triangle OpenUSD chess set took about 2 minutes to path-trace at 960×540 on those 2 cores; a typical
desktop CPU is several times faster.

*These figures are estimates: the app's memory use with large scenes hasn't been measured on a real machine, and the
per-model sizes are typical for Ollama's default 4-bit downloads.*
