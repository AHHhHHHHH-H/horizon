"""Render the sampler: every character's sample lines on each tier, from the pack's clips."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import cast  # noqa: E402
import tts  # noqa: E402

PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
OUT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/sampler")


def render(tier):
    if tier == "cpu":
        groups = [("en", [("m", "pocket-en", {"load_options": {"language": "english"}})], "cpu", 2),
                  ("fr", [("m", "pocket-fr", {"load_options": {"language": "french"}})], "cpu", 2)]
    else:
        key = {"high": "base17", "mid": "base06"}[tier]
        groups = [(None, [("m", key, {"session_options": {"qwen3_tts.voice_prompt_cache_slots": 10}})],
                   "metal", 4)]
    stats = {}
    for lang_only, entries, backend, threads in groups:
        s = tts.Server(f"{OUT}/srv-{tier}", entries, backend=backend, threads=threads,
                       voice_dir=PACK, max_loaded=1)
        try:
            s.speak({"model": "m", "input": "Ok.", "voice": "hz-veteran-en-neutral"})
            for cid, *_ in cast.CAST:
                for key, tone, en, fr in cast.SAMPLES:
                    for lang, text in (("en", en), ("fr", fr)):
                        if lang_only and lang != lang_only:
                            continue
                        out = f"{OUT}/{tier}/{cid}/{lang}-{key}.wav"
                        if os.path.exists(out):
                            continue
                        body = {"model": "m", "input": text, "voice": f"hz-{cid}-{lang}-{tone}",
                                "seed": 42}
                        if backend == "metal":
                            body["language"] = lang
                        r = s.speak(body, out)
                        r.pop("timing", None)
                        stats[f"{cid}/{lang}-{key}"] = r
                        print(tier, cid, lang, key, r, flush=True)
        finally:
            s.stop()
    json.dump(stats, open(f"{OUT}/{tier}.json", "w"), indent=1)


if __name__ == "__main__":
    for t in sys.argv[1:]:
        render(t)
