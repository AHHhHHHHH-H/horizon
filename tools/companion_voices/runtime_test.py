"""The runtime path test: every engine says the same 10 lines per character, streamed."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import cast  # noqa: E402
import tts  # noqa: E402

PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
OUT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/runtime")
SEEDS = {c[0]: c[6] for c in cast.CAST}
FOUR = ["veteran", "coach", "butler", "sunny"]


def run(engine, chars=None, lines=None, extra=None, threads=4):
    chars = chars or [c[0] for c in cast.CAST]
    lines = lines if lines is not None else list(enumerate(cast.RUNTIME_LINES))
    res_path = f"{OUT}/{engine}.json"
    res = json.load(open(res_path)) if os.path.exists(res_path) else {}
    if engine.startswith("pocket"):
        groups = [("en", "pocket-en", "english"), ("fr", "pocket-fr", "french")]
    else:
        groups = [(None, None, None)]
    for lang_only, key, load in groups:
        if engine.startswith("pocket"):
            entries = [("m", key, {"load_options": {"language": load},
                                   "session_options": {"pocket_tts.voice_state_cache_slots": 10}})]
            backend, th = "cpu", 2
        elif engine.startswith("base"):
            so = {"qwen3_tts.voice_prompt_cache_slots": 10}
            so.update(extra or {})
            entries = [("m", engine.split("-")[0], {"session_options": so})]
            backend, th = "metal", threads
        elif engine == "design":
            entries = [("m", "design", {"session_options": dict(extra or {})})]
            backend, th = "metal", threads
        elif engine == "vox":
            entries = [("m", "voxcpm2", {"session_options": dict(extra or {})})]
            backend, th = "metal", threads
        s = tts.Server(f"{OUT}/srv-{engine}", entries, backend=backend, threads=th,
                       voice_dir=PACK, max_loaded=1)
        try:
            warm = {"model": "m", "input": "Ok, let's go."}
            if engine.startswith(("base", "pocket")):
                warm["voice"] = f"hz-{chars[0]}-{lang_only or 'en'}-neutral"
            if engine == "vox":
                warm["voice_ref"] = f"{PACK}/hz-{chars[0]}-en-neutral.wav"
                warm["options"] = {"retry_badcase": False}
            s.speak(warm)
            for cid in chars:
                desc = next(c[5] for c in cast.CAST if c[0] == cid)
                for i, (lang, emo, text) in lines:
                    if lang_only and lang != lang_only:
                        continue
                    k = f"{cid}/{i:02d}"
                    if k in res:
                        continue
                    body = {"model": "m", "input": text}
                    ref_lang = lang
                    if engine.endswith(("xlang", "xvec")):
                        ref_lang = "en"
                    if engine.startswith(("base", "pocket")):
                        body["voice"] = f"hz-{cid}-{ref_lang}-{emo}"
                    if engine.endswith("xvec"):
                        body["options"] = {"x_vector_only_mode": True}
                    if not engine.startswith("pocket"):
                        body["language"] = lang
                    if engine == "design":
                        body["seed"] = SEEDS[cid]
                        body["options"] = {"instruct": desc + " " + cast.DESIGN_EMOTION[emo]}
                    if engine == "vox":
                        body["input"] = cast.VOX_STYLE.get(emo, "") + text
                        body["voice_ref"] = f"{PACK}/hz-{cid}-en-neutral.wav"
                        body["seed"] = 1234
                        body["options"] = {"retry_badcase": False}
                        body.pop("language")
                    r = s.speak(body, f"{OUT}/{engine}/{cid}/{i:02d}.wav")
                    r.pop("timing", None)
                    r.update({"lang": lang, "emo": emo})
                    res[k] = r
                    print(engine, k, r, flush=True)
                json.dump(res, open(res_path, "w"), indent=1)
            res[f"_footprint_mb_{lang_only or 'all'}"] = s.footprint_mb()
            res[f"_peak_mb_{lang_only or 'all'}"] = s.peak_mb()
        finally:
            s.stop()
        json.dump(res, open(res_path, "w"), indent=1)


if __name__ == "__main__":
    which = sys.argv[1:]
    for e in which:
        if e == "base17":
            run("base17")
        elif e == "base06":
            run("base06")
        elif e == "base17-memsaver":
            run("base17-memsaver", chars=["veteran", "sunny"],
                extra={"qwen3_tts.mem_saver": True})
        elif e == "base17-xlang":
            run("base17-xlang", lines=[(i, l) for i, l in enumerate(cast.RUNTIME_LINES)
                                       if l[0] == "fr"])
        elif e == "base17-xvec":
            run("base17-xvec", chars=FOUR, lines=[(i, l) for i, l in enumerate(cast.RUNTIME_LINES)
                                                  if l[0] == "fr"])
        elif e == "design":
            run("design", chars=FOUR)
        elif e == "vox":
            run("vox", chars=FOUR)
        elif e == "pocket":
            run("pocket")
