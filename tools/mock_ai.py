"""Mock Ollama + OpenRouter server for testing Rust Maya's AI layer offline.

Ollama endpoints at /api/*, OpenRouter at /api/v1/*.
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TABLE = {"reply": "Built a table with four legs.", "mel": [
    "polyCube -w 4 -h 0.2 -d 2 -n table_top", "move 0 2 0", "setColor wood",
    "polyCube -w 0.2 -h 2 -d 0.2 -n leg_1", "move 1.8 1 0.8",
    "polyCube -w 0.2 -h 2 -d 0.2 -n leg_2", "move -1.8 1 0.8",
    "polyCube -w 0.2 -h 2 -d 0.2 -n leg_3", "move 1.8 1 -0.8",
    "polyCube -w 0.2 -h 2 -d 0.2 -n leg_4", "move -1.8 1 -0.8",
    "select table_top leg_1 leg_2 leg_3 leg_4", "group", "rename table"]}

LOG = []

def last_user(body):
    msgs = [m for m in body.get("messages", []) if m.get("role") == "user"]
    if not msgs:
        return ""
    c = msgs[-1]["content"]
    if isinstance(c, list):  # OpenAI-style multimodal content
        return " ".join(p.get("text", "") for p in c if p.get("type") == "text")
    return c

def has_image(body):
    for m in body.get("messages", []):
        if m.get("images"):
            return True
        c = m.get("content")
        if isinstance(c, list) and any(p.get("type") == "image_url" for p in c):
            return True
    return False

VISION = ("gemma3", "llava", "qwen2.5vl", "openai/gpt-4o-mini")

def builder_answer(prompt, system, saw=False):
    if "look-development supervisor" in system:
        if saw:
            return {"reply": "I can see the render: the subject is lit only by the default light and looks flat.", "mel": ["lightRig -preset studio", "renderSettings -exp 0.3"]}
        return {"reply": "From the statistics: underexposed, add lights.", "mel": ["lightRig -preset studio"]}
    if "Rename the listed objects" in system:
        return {"reply": "Renamed.", "mel": ["rename pCube1 crate", "rename pSphere1 red_ball"]}
    if "colour palette" in system:
        return {"reply": "Sunset palette.", "mel": ["setColor 0.95 0.45 0.2 pCube1", "setColor 0.8 0.2 0.4 pSphere1"]}
    if "senior modeling TD" in system:
        return {"reply": "1. Rename default-named objects.\n2. Delete history before rigging.", "mel": ["delete -ch pCube1"]}
    if "MEL script failed" in system:
        return {"reply": "polyCub is misspelled.", "mel": ["polyCube"]}
    if "Explain what the given MEL" in system:
        return {"reply": "Creates a cube and moves it up one unit.", "mel": []}
    if "Write a MEL script" in system:
        return {"reply": "Stacks 3 cubes.", "mel": ["polyCube -n c1", "move 0 0.5 0", "polyCube -n c2", "move 0 1.5 0", "polyCube -n c3", "move 0 2.5 0"]}
    if "bounc" in prompt:
        return {"reply": "The ball bounces.", "mel": ["currentTime 1", "setKeyframe -at ty -v 1", "setKeyframe -t 12 -at ty -v 4", "setKeyframe -t 24 -at ty -v 1", "play"]}
    if "broken" in prompt:
        return {"reply": "Trying.", "mel": ["polyCub", "polyCube -n ok_cube"]}
    return TABLE

def chunks(text, n=7):
    for i in range(0, len(text), n):
        yield text[i:i+n]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass

    def _json(self, obj, code=200):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def _stream_start(self, ctype):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

    def _chunk(self, data):
        b = data.encode()
        self.wfile.write(b"%x\r\n%s\r\n" % (len(b), b))
        self.wfile.flush()

    def _end(self):
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        if self.path == "/api/tags":
            return self._json({"models": [{"name": "qwen3:8b"}, {"name": "tev1:latest"}, {"name": "nomic-embed-text"}, {"name": "gemma3:4b"}]})
        if self.path == "/api/ps":
            return self._json({"models": [{"name": "qwen3:8b", "size": 100, "size_vram": 100}]})
        if self.path == "/api/v1/models":
            return self._json({"data": [
                {"id": "openai/gpt-4o-mini", "supported_parameters": ["tools", "response_format"], "pricing": {"prompt": "0.00000015", "completion": "0.0000006"}, "context_length": 128000, "architecture": {"input_modalities": ["text", "image"]}},
                {"id": "meta-llama/llama-3.3-70b-instruct:free", "supported_parameters": ["tools"], "pricing": {"prompt": "0", "completion": "0"}, "context_length": 65536},
                {"id": "some/no-tools-model", "supported_parameters": [], "pricing": {"prompt": "0", "completion": "0"}}]})
        self._json({"error": "not found"}, 404)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        LOG.append((self.path, body.get("model"), "tools" in body))
        sys.stderr.write(f"POST {self.path} model={body.get('model')} tools={'tools' in body} stream={body.get('stream')}\n")
        prompt = last_user(body)
        system = body.get("messages", [{}])[0].get("content", "")
        if self.path == "/api/show":
            caps = ["completion", "tools"] if body.get("model", "").startswith("tev1") else ["completion"]
            return self._json({"capabilities": caps})
        if self.path == "/api/pull":
            self._stream_start("application/x-ndjson")
            for i in range(5):
                self._chunk(json.dumps({"status": "downloading", "completed": i * 20, "total": 100}) + "\n")
                time.sleep(0.05)
            self._chunk(json.dumps({"status": "success"}) + "\n")
            return self._end()
        img = has_image(body)
        if img and not body.get("model", "").startswith(VISION):
            return self._json({"error": f"model {body.get('model')} does not support images"}, 400)
        if self.path == "/api/chat":
            if "tools" in body:  # tev1 decision
                p = prompt.lower()
                if "?" in p or p.startswith("how"):
                    call = {"function": {"name": "answer", "arguments": {"text": "Press W for the Move tool, then drag an arrow."}}}
                elif "blue ball" in p:
                    call = {"function": {"name": "run_mel", "arguments": {"script": "polySphere -r 0.5 -n blue_ball; move 0 3 0; setColor blue", "reply": "Added a blue ball."}}}
                else:
                    call = {"function": {"name": "delegate_to_builder", "arguments": {"plan": "1. top 2. four legs 3. group"}}}
                return self._json({"message": {"role": "assistant", "content": "", "tool_calls": [call]}, "done": True})
            ans = json.dumps(builder_answer(prompt.lower(), system, img))
            if not body.get("stream"):
                return self._json({"message": {"role": "assistant", "content": ans}, "done": True})
            self._stream_start("application/x-ndjson")
            for c in chunks(ans):
                self._chunk(json.dumps({"message": {"content": c}, "done": False}) + "\n")
                time.sleep(0.01)
            self._chunk(json.dumps({"message": {"content": ""}, "done": True}) + "\n")
            return self._end()
        if self.path == "/api/v1/chat/completions":
            if self.headers.get("Authorization") != "Bearer sk-or-test":
                return self._json({"error": {"message": "No auth credentials found", "code": 401}}, 401)
            if "tools" in body:
                call = {"type": "function", "function": {"name": "delegate_to_builder", "arguments": json.dumps({"plan": "cloud plan"})}}
                return self._json({"choices": [{"message": {"role": "assistant", "content": None, "tool_calls": [call]}}]})
            ans = json.dumps(builder_answer(prompt.lower(), system, img))
            self._stream_start("text/event-stream")
            self._chunk(": OPENROUTER PROCESSING\n\n")
            for c in chunks(ans):
                self._chunk("data: " + json.dumps({"choices": [{"delta": {"content": c}}]}) + "\n\n")
                time.sleep(0.01)
            self._chunk("data: [DONE]\n\n")
            return self._end()
        self._json({"error": "not found"}, 404)

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
