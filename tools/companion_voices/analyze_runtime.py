"""Score the runtime test: latency, speed, memory, speaker consistency, identification, WER,
expressiveness per engine. Writes runtime/summary.json."""
import glob
import json
import os
import statistics as st
import sys

sys.path.insert(0, os.path.dirname(__file__))
import cast  # noqa: E402
import spk  # noqa: E402
import tts  # noqa: E402

OUT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/runtime")
PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
ENGINES = ["base17", "base06", "base17-xlang", "base17-memsaver", "base17-xvec", "design", "vox", "pocket"]


def pct(xs, p):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(p * len(xs)))], 3) if xs else None


def main():
    anchors = {c[0]: spk.emb(f"{PACK}/hz-{c[0]}-en-neutral.wav") for c in cast.CAST}
    wer_path = f"{OUT}/wer.json"
    wers = json.load(open(wer_path)) if os.path.exists(wer_path) else {}
    todo = [p for e in ENGINES for p in glob.glob(f"{OUT}/{e}/*/*.wav")
            if os.path.relpath(p, OUT) not in wers]
    if todo:
        s = tts.Server(OUT + "/srv-asr", [("asr", "parakeet", {"task": "asr", "mode": "offline"})],
                       backend="cpu", threads=4)
        try:
            for p in todo:
                i = int(os.path.basename(p)[:2])
                ref = cast.RUNTIME_LINES[i][2]
                wers[os.path.relpath(p, OUT)] = tts.wer(ref, tts.asr(s, p))
        finally:
            s.stop()
        json.dump(wers, open(wer_path, "w"), indent=1)
    summary = {}
    for e in ENGINES:
        rp = f"{OUT}/{e}.json"
        if not os.path.exists(rp):
            continue
        res = json.load(open(rp))
        rows = {k: v for k, v in res.items() if not k.startswith("_")}
        chars = sorted({k.split("/")[0] for k in rows})
        sims, ident, own_minus_other, w = [], 0, [], []
        per_char_min = []
        loud, voiced = [], []
        for cid in chars:
            cs = []
            by_emo = {}
            for k, v in rows.items():
                if not k.startswith(cid + "/"):
                    continue
                p = f"{OUT}/{e}/{k}.wav"
                em = spk.emb(p)
                own = spk.cos(em, anchors[cid])
                others = [spk.cos(em, anchors[o]) for o in anchors if o != cid]
                cs.append(own)
                ident += own > max(others)
                own_minus_other.append(own - max(others))
                w.append(wers.get(f"{e}/{k}.wav", 1.0))
                by_emo.setdefault(v["emo"], []).append(spk.stats(p))
            sims += cs
            per_char_min.append(min(cs))
            if "neutral" in by_emo and "shout" in by_emo:
                loud.append(st.mean(x["rms_db"] for x in by_emo["shout"])
                            - st.mean(x["rms_db"] for x in by_emo["neutral"]))
            if "neutral" in by_emo and "whisper" in by_emo:
                voiced.append(st.mean(x["voiced"] for x in by_emo["whisper"])
                              / max(1e-6, st.mean(x["voiced"] for x in by_emo["neutral"])))
        ttfa = [v["ttfa"] for v in rows.values()]
        rtf = [v["rtf"] for v in rows.values()]
        summary[e] = {
            "lines": len(rows), "characters": len(chars),
            "first_audio_p50_s": pct(ttfa, 0.5), "first_audio_p90_s": pct(ttfa, 0.9),
            "first_audio_min_s": pct(ttfa, 0.0),
            "rtf_p50": pct(rtf, 0.5),
            "similarity_to_anchor_mean": round(st.mean(sims), 3),
            "similarity_worst_line_mean_over_chars": round(st.mean(per_char_min), 3),
            "identified_as_own_character": f"{ident}/{len(rows)}",
            "margin_own_minus_best_other": round(st.mean(own_minus_other), 3),
            "wer_mean": round(st.mean(w), 3),
            "shout_minus_neutral_db": round(st.mean(loud), 1) if loud else None,
            "whisper_voicing_vs_neutral": round(st.mean(voiced), 2) if voiced else None,
            "memory": {k: v for k, v in res.items() if k.startswith("_")},
        }
        print(e, json.dumps(summary[e]), flush=True)
    json.dump(summary, open(f"{OUT}/summary.json", "w"), indent=1)


if __name__ == "__main__":
    main()
