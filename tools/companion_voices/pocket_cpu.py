import sys, os, glob, json
sys.path.insert(0, os.path.dirname(__file__))
import tts, cast
OUT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/cpu")
res = {}
for lang, key, load in (("en", "pocket-en", "english"), ("fr", "pocket-fr", "french")):
    emb = os.path.join(os.path.dirname(tts.MODELS[key]), "embeddings")
    voices = sorted(os.path.basename(p)[:-12] for p in glob.glob(emb + "/*.safetensors"))
    s = tts.Server(OUT + "/srv-" + lang, [("p", key, {"load_options": {"language": load}})],
                   backend="cpu", threads=2)
    try:
        s.speak({"model": "p", "input": "Ok.", "voice": voices[0]})
        for v in voices:
            for k, tone, en, fr in cast.SAMPLES:
                text = en if lang == "en" else fr
                r = s.speak({"model": "p", "input": text, "voice": v, "seed": 42},
                            f"{OUT}/{lang}/{v}-{k}.wav")
                res[f"{lang}/{v}-{k}"] = r
                print(lang, v, k, r["ttfa"], r["rtf"], flush=True)
        res[f"{lang}/footprint_mb"] = s.footprint_mb()
    finally:
        s.stop()
json.dump(res, open(OUT + "/results.json", "w"), indent=1)
