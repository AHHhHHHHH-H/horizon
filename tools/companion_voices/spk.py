"""Speaker embeddings and simple acoustic stats for WAV files (measurement only, never shipped).

Usage as a module: emb(path) -> unit vector (WeSpeaker ResNet34, 16 kHz); stats(path) -> dict.
"""
import functools
import os

import numpy as np
import sherpa_onnx
import soundfile as sf
from scipy.signal import resample_poly

MODELS = os.path.expanduser("~/builds/horizon-companion-scratch/spk-models")


@functools.lru_cache(maxsize=2)
def extractor(name="wespeaker_en_voxceleb_resnet34_LM.onnx"):
    cfg = sherpa_onnx.SpeakerEmbeddingExtractorConfig(
        model=os.path.join(MODELS, name), num_threads=2, provider="cpu"
    )
    return sherpa_onnx.SpeakerEmbeddingExtractor(cfg)


def load16k(path):
    x, sr = sf.read(path, dtype="float32", always_2d=False)
    if x.ndim > 1:
        x = x.mean(axis=1)
    if sr != 16000:
        g = np.gcd(sr, 16000)
        x = resample_poly(x, 16000 // g, sr // g).astype(np.float32)
    return x


def emb(path, name="wespeaker_en_voxceleb_resnet34_LM.onnx"):
    ex = extractor(name)
    x = load16k(path)
    s = ex.create_stream()
    s.accept_waveform(16000, x)
    s.input_finished()
    v = np.array(ex.compute(s), dtype=np.float32)
    return v / (np.linalg.norm(v) + 1e-9)


def cos(a, b):
    return float(np.dot(a, b))


def f0_track(x, sr=16000, fmin=60.0, fmax=500.0, hop=160, win=640):
    """Voiced frames' F0 by normalised autocorrelation (crude but stable)."""
    lo, hi = int(sr / fmax), int(sr / fmin)
    out = []
    for i in range(0, len(x) - win, hop):
        f = x[i : i + win] - x[i : i + win].mean()
        e = float(np.dot(f, f))
        if e < 1e-4:
            continue
        ac = np.correlate(f, f, "full")[win - 1 :]
        ac = ac / (ac[0] + 1e-9)
        k = lo + int(np.argmax(ac[lo:hi]))
        if ac[k] > 0.45:
            out.append(sr / k)
    return np.array(out)


def stats(path):
    x = load16k(path)
    dur = len(x) / 16000
    rms = float(np.sqrt(np.mean(x**2)) + 1e-9)
    f0 = f0_track(x)
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x)))) if len(x) else np.zeros(1)
    freqs = np.fft.rfftfreq(len(x), 1 / 16000)
    centroid = float((spec * freqs).sum() / (spec.sum() + 1e-9))
    return {
        "dur": round(dur, 2),
        "rms_db": round(float(20 * np.log10(rms)), 1),
        "f0_med": round(float(np.median(f0)), 1) if len(f0) else 0.0,
        "voiced": round(len(f0) * 160 / 16000 / max(dur, 1e-9), 2),
        "centroid": round(centroid),
    }
