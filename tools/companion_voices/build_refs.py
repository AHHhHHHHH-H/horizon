"""Design the cast's reference clips.

Phase design: VoiceDesign candidates (description + emotion, seeds 1-3) for every character x
language x emotion. The English neutral anchor is the medoid of the character's neutral seeds.
Phase vox: VoxCPM2 controllable clones of the anchor (style prefix, 2 seeds) for every other
language x emotion. Phase asr: Parakeet transcript + WER of every candidate.
Phase pick: per character x language x emotion, the candidate with the best speaker similarity to
the anchor among those with a correct transcript, a usable length and the emotion's acoustic
signature. Output: the voice pack staging folder with the WAVs and manifest.json.
"""
import json
import os
import shutil
import sys
import wave

sys.path.insert(0, os.path.dirname(__file__))
import cast  # noqa: E402

ROOT = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work/cand")
PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
SCORES = os.path.join(ROOT, "scores.json")
SEEDS = [1, 2, 3]
VOX_SEEDS = [1234]


def seeds_for(lang, emo):
    """Three takes for the anchor (English neutral), two for every other clip."""
    return SEEDS if (lang, emo) == ("en", "neutral") else SEEDS[:2]
LANGS = ["en", "fr"]


def cand_path(c, lang, emo, route, seed):
    return f"{ROOT}/{c}/{lang}-{emo}-{route}{seed}.wav"


def duration(p):
    with wave.open(p) as w:
        return w.getnframes() / w.getframerate()


def phase_design(only=None):
    import tts
    s = tts.Server(ROOT + "/srv-design", [("design", "design", {"mode": "offline"})], threads=4)
    try:
        for cid, _l, _g, _d, _p, desc, _seed in cast.CAST:
            if only and cid not in only:
                continue
            for lang in LANGS:
                for emo in cast.EMOTIONS:
                    for seed in seeds_for(lang, emo):
                        out = cand_path(cid, lang, emo, "vd", seed)
                        if os.path.exists(out):
                            continue
                        r = tts.speak_offline(s, {
                            "model": "design", "input": cast.REF_TEXT[lang][emo], "language": lang,
                            "seed": seed,
                            "options": {"instruct": desc + " " + cast.DESIGN_EMOTION[emo]}}, out)
                        print("vd", cid, lang, emo, seed, r, flush=True)
        print("design footprint MB", s.footprint_mb(), "peak", s.peak_mb())
    finally:
        s.stop()


def anchors():
    """The English neutral seed most similar to the character's other VoiceDesign clips."""
    import spk
    out = {}
    for cid, *_ in cast.CAST:
        neutral = [cand_path(cid, "en", "neutral", "vd", s) for s in SEEDS]
        others = [cand_path(cid, lang, e, "vd", s) for lang in LANGS for e in cast.EMOTIONS
                  for s in seeds_for(lang, e)]
        E = {p: spk.emb(p) for p in set(neutral + others) if os.path.exists(p)}
        best = max(neutral, key=lambda a: sum(spk.cos(E[a], E[o]) for o in others if o != a))
        out[cid] = best
    return out


def phase_vox(anc, only=None):
    import tts
    s = tts.Server(ROOT + "/srv-vox", [("vox", "voxcpm2", {"mode": "offline"})], threads=4)
    try:
        for cid, *_ in cast.CAST:
            if only and cid not in only:
                continue
            for lang in LANGS:
                for emo in cast.EMOTIONS:
                    if lang == "en" and emo == "neutral":
                        continue
                    for seed in VOX_SEEDS:
                        out = cand_path(cid, lang, emo, "vx", seed)
                        if os.path.exists(out):
                            continue
                        text = cast.VOX_STYLE.get(emo, "") + cast.REF_TEXT[lang][emo]
                        r = tts.speak_offline(s, {"model": "vox", "input": text,
                                                  "voice_ref": anc[cid], "seed": seed}, out)
                        print("vx", cid, lang, emo, seed, r, flush=True)
        print("vox footprint MB", s.footprint_mb(), "peak", s.peak_mb())
    finally:
        s.stop()


def all_cands():
    for cid, *_ in cast.CAST:
        for lang in LANGS:
            for emo in cast.EMOTIONS:
                for route, seeds in (("vd", seeds_for(lang, emo)), ("vx", VOX_SEEDS)):
                    for seed in seeds:
                        p = cand_path(cid, lang, emo, route, seed)
                        if os.path.exists(p):
                            yield cid, lang, emo, route, seed, p


def phase_asr():
    import tts
    scores = json.load(open(SCORES)) if os.path.exists(SCORES) else {}
    s = tts.Server(ROOT + "/srv-asr", [("asr", "parakeet", {"task": "asr", "mode": "offline"})],
                   threads=4)
    try:
        for cid, lang, emo, route, seed, p in all_cands():
            k = os.path.relpath(p, ROOT)
            if k in scores and "asr" in scores[k]:
                continue
            text = tts.asr(s, p)
            scores.setdefault(k, {}).update(
                {"asr": text, "wer": tts.wer(cast.REF_TEXT[lang][emo], text)})
    finally:
        s.stop()
    json.dump(scores, open(SCORES, "w"), indent=1, ensure_ascii=False)


def acoustic_ok(emo, st, a):
    if emo == "shout":
        return st["rms_db"] >= a["rms_db"] + 2 or st["f0_med"] >= a["f0_med"] * 1.25
    if emo == "whisper":
        return st["voiced"] <= a["voiced"] * 0.6 or st["rms_db"] <= a["rms_db"] - 4
    return True


def phase_pick(anc):
    import spk
    scores = json.load(open(SCORES))
    picks = {}
    for cid, *_ in cast.CAST:
        a = anc[cid]
        ea, sa = spk.emb(a), spk.stats(a)
        for lang in LANGS:
            for emo in cast.EMOTIONS:
                rows = []
                for c2, l2, e2, route, seed, p in all_cands():
                    if (c2, l2, e2) != (cid, lang, emo):
                        continue
                    k = os.path.relpath(p, ROOT)
                    sc = scores[k]
                    st = spk.stats(p)
                    sc.update({"sim": round(spk.cos(ea, spk.emb(p)), 3), "stats": st})
                    limit = 0.3 if emo == "laugh" else 0.12
                    ok = (sc["wer"] <= limit and 3.5 <= st["dur"] <= 9.5
                          and acoustic_ok(emo, st, sa))
                    rows.append((ok, sc["sim"], route, seed, p, k))
                if lang == "en" and emo == "neutral":
                    pick = next(r for r in rows if r[4] == a)
                else:
                    good = [r for r in rows if r[0]]
                    if good:
                        pick = max(good, key=lambda r: r[1])
                    elif emo == "whisper":
                        # A real whisper first: the least voiced take with a correct transcript.
                        fine = [r for r in rows if scores[r[5]]["wer"] <= 0.12] or rows
                        pick = min(fine, key=lambda r: scores[r[5]]["stats"]["voiced"])
                    else:
                        pick = max(rows, key=lambda r: r[1])
                picks[(cid, lang, emo)] = {"file": pick[4], "route": pick[2], "seed": pick[3],
                                           "sim": pick[1], "checked": pick[0]}
                print(cid, lang, emo, pick[2], pick[3], "sim", pick[1],
                      "" if pick[0] else "UNCHECKED", " | cands:",
                      " ".join(f"{r[2]}{r[3]}:{r[1]:.2f}{'' if r[0] else 'x'}" for r in rows),
                      flush=True)
    json.dump(scores, open(SCORES, "w"), indent=1, ensure_ascii=False)
    return picks


def trim_copy(src, dst, pad=0.12, thresh=0.01):
    """Copy a WAV without its leading and trailing silence."""
    import numpy as np
    with wave.open(src) as w:
        rate = w.getframerate()
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
    f = np.abs(x.astype(np.float32) / 32768)
    loud = np.where(f > thresh)[0]
    if len(loud):
        a = max(0, loud[0] - int(pad * rate))
        b = min(len(x), loud[-1] + int(pad * rate))
        x = x[a:b]
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with wave.open(dst, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(x.tobytes())


def write_pack(picks):
    if os.path.isdir(PACK):
        shutil.rmtree(PACK)
    os.makedirs(PACK)
    voices = []
    for cid, label, gender, short, personality, desc, seed in cast.CAST:
        clips = []
        for lang in LANGS:
            for emo in cast.EMOTIONS:
                pk = picks[(cid, lang, emo)]
                name = f"hz-{cid}-{lang}-{emo}"
                trim_copy(pk["file"], f"{PACK}/{name}.wav")
                clips.append({"emotion": emo, "language": lang, "file": f"{name}.wav",
                              "text": cast.REF_TEXT[lang][emo],
                              "seconds": round(duration(f"{PACK}/{name}.wav"), 2),
                              "made_with": "qwen3-tts-1.7b-voicedesign" if pk["route"] == "vd"
                              else "voxcpm2-clone-of-anchor",
                              "seed": pk["seed"], "similarity_to_anchor": pk["sim"],
                              "checked": pk["checked"]})
        voices.append({"id": cid, "label": label, "gender": gender, "description": short,
                       "personality": personality, "design": desc, "clips": clips})
    with open(f"{PACK}/prompt_text", "w") as f:
        for v in voices:
            for c in v["clips"]:
                f.write(f"{c['file'][:-4]}|{c['text']}\n")
    json.dump({"version": 1, "voices": voices}, open(f"{PACK}/manifest.json", "w"), indent=1,
              ensure_ascii=False)
    print("pack", PACK)


if __name__ == "__main__":
    step = sys.argv[1]
    only = sys.argv[2:] or None
    if step == "design":
        phase_design(only)
    elif step == "vox":
        a = anchors()
        json.dump(a, open(ROOT + "/anchors.json", "w"), indent=1)
        phase_vox(a, only)
    elif step == "asr":
        phase_asr()
    elif step == "pick":
        a = json.load(open(ROOT + "/anchors.json"))
        write_pack(phase_pick(a))
