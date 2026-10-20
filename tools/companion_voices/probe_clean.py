import sys, os, json, statistics as st
sys.path.insert(0, os.path.dirname(__file__))
import tts, cast
PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
OUT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/probe")
CH = ["veteran", "sunny", "butler"]
res = {}
for eng in ["base17", "base06", "design"]:
    so = {"qwen3_tts.voice_prompt_cache_slots": 10}
    s = tts.Server(f"{OUT}/srv-{eng}", [("m", eng, {"session_options": so})], threads=4, voice_dir=PACK, max_loaded=1)
    try:
        s.speak({"model": "m", "input": "Ok.", "voice": "hz-veteran-en-neutral"} if eng != "design" else {"model": "m", "input": "Ok."})
        rows = []
        prep = []
        for cid in CH:
            desc = next(c[5] for c in cast.CAST if c[0] == cid); seed = next(c[6] for c in cast.CAST if c[0] == cid)
            if eng != "design":
                for lang in ["en", "fr"]:
                    for emo in cast.EMOTIONS:
                        r = s.speak({"model": "m", "input": "Ok.", "voice": f"hz-{cid}-{lang}-{emo}", "language": lang})
                        prep.append(r["total"])
            for i, (lang, emo, text) in enumerate(cast.RUNTIME_LINES):
                b = {"model": "m", "input": text, "language": lang}
                if eng == "design":
                    b.update({"seed": seed, "options": {"instruct": desc + " " + cast.DESIGN_EMOTION[emo]}})
                else:
                    b["voice"] = f"hz-{cid}-{lang}-{emo}"
                r = s.speak(b, f"{OUT}/{eng}/{cid}-{i:02d}.wav")
                rows.append(r)
        res[eng] = {"first_audio_p50": round(st.median(r["ttfa"] for r in rows), 3),
                    "first_audio_max": round(max(r["ttfa"] for r in rows), 3),
                    "rtf_p50": round(st.median(r["rtf"] for r in rows), 3),
                    "prepare_per_clip_s": round(st.median(prep), 3) if prep else None,
                    "footprint_mb": s.footprint_mb(), "peak_mb": s.peak_mb()}
        print(eng, res[eng], flush=True)
    finally:
        s.stop()
json.dump(res, open(f"{OUT}/clean.json", "w"), indent=1)
