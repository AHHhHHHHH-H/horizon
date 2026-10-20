"""Convert the sampler's WAVs to MP3 and write index.html (no audio is played)."""
import glob
import html
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(__file__))
import cast  # noqa: E402

WORK = os.path.expanduser("~/builds/horizon-companion-scratch/voices-work")
DEMO = os.path.expanduser("~/builds/horizon-companion-demo/voices")
PACK = os.path.expanduser("~/builds/horizon-companion-voices/pack")
# Built-in Pocket voices left out of the game (licence): not in the sampler either.
EXCLUDED = {"cosette", "jean", "juergen", "rafael"}
TIERS = [("high", "High-end GPU (Qwen3-TTS 1.7B Base)"), ("mid", "Mid GPU (Qwen3-TTS 0.6B Base)"),
         ("cpu", "Processor (Pocket TTS, cloned)")]


def mp3(src, dst):
    if os.path.exists(dst) and os.path.getmtime(dst) >= os.path.getmtime(src):
        return
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    subprocess.run(["lame", "--quiet", "-V", "4", src, dst], check=True)


def player(rel):
    if not os.path.exists(os.path.join(DEMO, rel)):
        return '<span class="missing">-</span>'
    return f'<audio controls preload="none" src="{html.escape(rel)}"></audio>'


def main():
    manifest = json.load(open(f"{PACK}/manifest.json")) if os.path.exists(
        f"{PACK}/manifest.json") else {"voices": []}
    clips = {v["id"]: v["clips"] for v in manifest["voices"]}
    # Character renders: WORK/sampler/<tier>/<char>/<lang>-<line>.wav; references from the pack.
    for wav in glob.glob(f"{WORK}/sampler/*/*/*.wav"):
        tier, cid, name = wav.split("/")[-3:]
        mp3(wav, f"{DEMO}/{cid}/{tier}-{name[:-4]}.mp3")
    for wav in glob.glob(f"{PACK}/hz-*.wav"):
        _, cid, lang, emo = os.path.basename(wav)[:-4].split("-")
        mp3(wav, f"{DEMO}/{cid}/ref-{lang}-{emo}.mp3")
    for wav in glob.glob(f"{WORK}/cpu/*/*.wav"):
        lang, name = wav.split("/")[-2:]
        if name.rsplit("-", 1)[0] in EXCLUDED:
            continue
        mp3(wav, f"{DEMO}/cpu/{lang}/{name[:-4]}.mp3")
    results = json.load(open(f"{WORK}/runtime/summary.json")) if os.path.exists(
        f"{WORK}/runtime/summary.json") else {}

    out = ["<!doctype html><meta charset=utf-8><title>Horizon companion voices</title>",
           "<style>body{font:14px system-ui;margin:24px;max-width:1500px}h2{margin-top:36px}"
           "table{border-collapse:collapse}td,th{border:1px solid #ddd;padding:4px 6px;"
           "vertical-align:middle}th{background:#f4f4f4;text-align:left}audio{height:28px;"
           "width:190px}.d{color:#555}.missing{color:#bbb}nav a{margin-right:10px}"
           "details{margin:8px 0}</style>",
           "<h1>Horizon companion voices</h1>",
           "<p>Designed voices (Qwen3-TTS VoiceDesign, no real person). Each character has one "
           "reference clip per emotion and language; at run time the clip matching the line's tone "
           "is cloned. Columns are the hardware tiers. See README.md for engines and numbers.</p>"]
    out.append("<nav>" + " ".join(f'<a href="#{c[0]}">{html.escape(c[1])}</a>' for c in cast.CAST)
               + ' <a href="#cpu">Pocket TTS built-in voices</a></nav>')
    for cid, label, gender, short, personality, desc, _seed in cast.CAST:
        dflt = f" · default for <b>{personality}</b>" if personality else ""
        out.append(f'<h2 id="{cid}">{html.escape(label)} <span class="d">— {html.escape(short)}'
                   f'{dflt}</span></h2>')
        out.append(f'<p class="d">{"Woman" if gender == "f" else "Man"}. Designed from: '
                   f'<i>{html.escape(desc)}</i></p>')
        out.append("<table><tr><th>Line</th>" + "".join(
            f"<th>{lang.upper()} · {html.escape(t[1])}</th>" for lang in ("en", "fr")
            for t in TIERS) + "</tr>")
        for key, tone, en, fr in cast.SAMPLES:
            out.append(f"<tr><td><b>{key}</b> ({tone})<br><span class=d>{html.escape(en)}<br>"
                       f"{html.escape(fr)}</span></td>" + "".join(
                f"<td>{player(f'{cid}/{t[0]}-{lang}-{key}.mp3')}</td>" for lang in ("en", "fr")
                for t in TIERS) + "</tr>")
        out.append("</table>")
        out.append("<details><summary>Reference clips (what the cloning models hear)</summary>"
                   "<table><tr><th>Emotion</th><th>EN</th><th>FR</th></tr>")
        for emo in cast.EMOTIONS:
            made = {c["language"]: c for c in clips.get(cid, []) if c["emotion"] == emo}
            cell = lambda lang: player(f"{cid}/ref-{lang}-{emo}.mp3") + (
                f'<br><span class=d>{html.escape(made[lang]["made_with"])}, similarity '
                f'{made[lang]["similarity_to_anchor"]}</span>' if lang in made else "")
            out.append(f"<tr><td>{emo}</td><td>{cell('en')}</td><td>{cell('fr')}</td></tr>")
        out.append("</table></details>")
    out.append('<h2 id="cpu">Pocket TTS built-in voices (processor, no cloning)</h2>')
    for lang in ("en", "fr"):
        voices = sorted({os.path.basename(p).rsplit("-", 1)[0]
                         for p in glob.glob(f"{DEMO}/cpu/{lang}/*.mp3")} - EXCLUDED)
        out.append(f"<h3>{lang.upper()}</h3><table><tr><th>Voice</th>" + "".join(
            f"<th>{k}</th>" for k, *_ in cast.SAMPLES) + "</tr>")
        for v in voices:
            out.append(f"<tr><td>{v}</td>" + "".join(
                f"<td>{player(f'cpu/{lang}/{v}-{k}.mp3')}</td>" for k, *_ in cast.SAMPLES)
                + "</tr>")
        out.append("</table>")
    if results:
        out.append("<h2>Measurements</h2><pre>" + html.escape(json.dumps(results, indent=1))
                   + "</pre>")
    open(f"{DEMO}/index.html", "w").write("\n".join(out))
    print("wrote", f"{DEMO}/index.html")


if __name__ == "__main__":
    main()
