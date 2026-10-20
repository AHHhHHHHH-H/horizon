"""audio.cpp server driver for the voice work: start a server from a config, stream a line, save a WAV.

Stdlib only. Never plays audio.
"""
import base64
import json
import os
import socket
import subprocess
import time
import urllib.request
import wave

RUNTIME = os.path.expanduser(
    "~/builds/horizon-companion-scratch/data/companion/voice-runtime/v0.9.2-macos-aarch64-metal"
)
DL = os.path.expanduser("~/builds/horizon-companion-scratch/voices-dl")
MODELS = {
    "design": f"{DL}/Qwen3-TTS-12Hz-1.7B-VoiceDesign-GGUF/qwen3-tts-12hz-1.7b-voicedesign-q8_0.gguf",
    "base17": f"{DL}/Qwen3-TTS-12Hz-1.7B-Base-GGUF/qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
    "base06": f"{DL}/Qwen3-TTS-12Hz-0.6B-Base-GGUF/qwen3-tts-12hz-0.6b-base-q8_0.gguf",
    "voxcpm2": f"{DL}/VoxCPM2-GGUF/voxcpm2-q8_0.gguf",
    "pocket-en": f"{DL}/PocketTTS-GGUF/english/pocket-tts-english-q8_0.gguf",
    "pocket-fr": f"{DL}/PocketTTS-GGUF/french/pocket-tts-french-q8_0.gguf",
    "parakeet": os.path.expanduser("~/builds/horizon-companion-scratch/data/companion/models/"
                                   "parakeet-tdt-0.6b-v3-q8_0/parakeet-tdt-0.6b-v3-q8_0.gguf"),
}
FAMILY = {"design": "qwen3_tts", "base17": "qwen3_tts", "base06": "qwen3_tts",
          "voxcpm2": "voxcpm2", "pocket-en": "pocket_tts", "pocket-fr": "pocket_tts",
          "parakeet": "parakeet_tdt"}


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


class Server:
    def __init__(self, work, entries, backend="metal", threads=4, voice_dir=None, max_loaded=0):
        """entries: list of (id, model_key, extra dict merged into the model entry)."""
        os.makedirs(work, exist_ok=True)
        self.port = free_port()
        models = []
        for mid, key, extra in entries:
            e = {"id": mid, "family": FAMILY[key], "path": MODELS[key], "task": "tts",
                 "mode": "streaming"}
            e.update(extra or {})
            models.append(e)
        cfg = {"host": "127.0.0.1", "port": self.port, "backend": backend, "threads": threads,
               "lazy_load": True, "log_request_body": False, "max_loaded_models": max_loaded,
               "models": models}
        if voice_dir:
            cfg["voice_dir"] = voice_dir
        self.cfg_path = os.path.join(work, "server.json")
        json.dump(cfg, open(self.cfg_path, "w"), indent=1)
        self.log = open(os.path.join(work, "server.log"), "a")
        self.proc = subprocess.Popen(
            [os.path.join(RUNTIME, "audiocpp_server"), "--config", self.cfg_path, "--no-ui",
             "--log"], cwd=RUNTIME, stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(200):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{self.port}/health", timeout=1)
                return
            except Exception:
                time.sleep(0.1)
        raise RuntimeError("server did not start")

    def footprint_mb(self):
        """Physical footprint (includes Metal buffers on Apple silicon)."""
        try:
            out = subprocess.run(["footprint", "-p", str(self.proc.pid)], capture_output=True,
                                 text=True, timeout=20).stdout
            for line in out.splitlines():
                if "phys_footprint:" in line and "peak" not in line:
                    v = line.split("phys_footprint:")[1].strip().split()
                    n = float(v[0])
                    unit = v[1].upper() if len(v) > 1 else "B"
                    return round(n * {"KB": 1 / 1024, "MB": 1, "GB": 1024, "B": 1 / 2**20}.get(unit, 1))
        except Exception:
            pass
        return None

    def peak_mb(self):
        try:
            out = subprocess.run(["footprint", "-p", str(self.proc.pid)], capture_output=True,
                                 text=True, timeout=20).stdout
            for line in out.splitlines():
                if "phys_footprint_peak:" in line:
                    v = line.split("phys_footprint_peak:")[1].strip().split()
                    n = float(v[0])
                    unit = v[1].upper() if len(v) > 1 else "B"
                    return round(n * {"KB": 1 / 1024, "MB": 1, "GB": 1024, "B": 1 / 2**20}.get(unit, 1))
        except Exception:
            pass
        return None

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except Exception:
            self.proc.kill()

    def speak(self, body, out_wav=None, rate=24000, timeout=300):
        body = dict(body)
        body.setdefault("response_format", "pcm")
        body.setdefault("stream_format", "sse")
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/v1/audio/speech", data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json", "Accept": "text/event-stream"})
        t0 = time.perf_counter()
        first = None
        pcm = bytearray()
        done = False
        timing = None
        with urllib.request.urlopen(req, timeout=timeout) as r:
            for raw in r:
                line = raw.decode().strip()
                if not line.startswith("data:"):
                    continue
                data = line[5:].strip()
                if data == "[DONE]":
                    break
                ev = json.loads(data)
                t = ev.get("type")
                if t == "speech.audio.delta":
                    chunk = base64.b64decode(ev.get("audio", ""))
                    if chunk and first is None:
                        first = time.perf_counter() - t0
                    pcm += chunk
                elif t == "speech.audio.done":
                    done = True
                    timing = ev.get("timing")
                elif "error" in ev:
                    raise RuntimeError(json.dumps(ev["error"]))
        total = time.perf_counter() - t0
        if not done:
            raise RuntimeError("stream ended early")
        secs = len(pcm) / 2 / rate
        if out_wav:
            os.makedirs(os.path.dirname(out_wav), exist_ok=True)
            with wave.open(out_wav, "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(rate)
                w.writeframes(bytes(pcm))
        return {"ttfa": round(first or 0, 3), "total": round(total, 3), "secs": round(secs, 2),
                "rtf": round(total / max(secs, 1e-6), 3), "timing": timing}


def speak_offline(server, body, out_wav, timeout=600):
    """Offline-mode entry: the whole WAV in one response."""
    body = dict(body)
    body["response_format"] = "wav"
    body.pop("stream_format", None)
    req = urllib.request.Request(
        f"http://127.0.0.1:{server.port}/v1/audio/speech", data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"})
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        data = r.read()
    total = time.perf_counter() - t0
    os.makedirs(os.path.dirname(out_wav), exist_ok=True)
    open(out_wav, "wb").write(data)
    with wave.open(out_wav) as w:
        secs = w.getnframes() / w.getframerate()
    return {"total": round(total, 3), "secs": round(secs, 2), "rtf": round(total / max(secs, 1e-6), 3)}


def asr(server, wav_path, model="asr", language=None):
    b = "hzb7f3a9c1e5"
    parts = [f"--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n".encode()]
    if language:
        parts.append(f"--{b}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n{language}\r\n".encode())
    parts.append(f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n".encode())
    parts.append(open(wav_path, "rb").read())
    parts.append(f"\r\n--{b}--\r\n".encode())
    req = urllib.request.Request(f"http://127.0.0.1:{server.port}/v1/audio/transcriptions",
                                 data=b"".join(parts),
                                 headers={"Content-Type": f"multipart/form-data; boundary={b}"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read()).get("text", "").strip()


def _norm(t):
    import re
    import unicodedata
    t = unicodedata.normalize("NFC", t.lower()).replace("\u2019", "'")
    t = re.sub(r"[^\w' ]+", " ", t)
    return [w for w in t.split() if w not in ("ha", "haha", "hahaha", "ah", "hah", "heh")]


def wer(ref, hyp):
    r, h = _norm(ref), _norm(hyp)
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            cur = d[j]
            d[j] = min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
            prev = cur
    return round(d[len(h)] / max(len(r), 1), 3)
