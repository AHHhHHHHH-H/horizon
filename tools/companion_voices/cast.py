"""The companion voice cast: designed voices (Qwen3-TTS VoiceDesign), no real person.

Dev tools that design the cast and build the voice pack (docs/companions.md, "The voice cast"). They
run on the Mac mini against audio.cpp's server and keep their work in ~/builds:
  dl_voices.sh             the models at the pinned audio.cpp-gguf revision, SHA-256 checked
  build_refs.py design|vox|asr|pick   candidates, transcripts, the picks and the pack
                           (~/builds/horizon-companion-voices/pack: WAVs, prompt_text, manifest.json)
  runtime_test.py, analyze_runtime.py  the engines on the same lines: speed, memory, consistency
  pocket_cpu.py, render_sampler.py, make_index.py  the listening sampler
spk.py (speaker embeddings, sherpa-onnx + WeSpeaker) needs a venv with sherpa-onnx, numpy,
soundfile and scipy; the rest is stdlib. Nothing here plays audio. Keep in step with
crates/horizon-senses/src/cast.rs (ids, descriptions, seeds, transcripts).
"""

# id, label, gender, short description, default personality (or None), VoiceDesign description, seed
CAST = [
    ("veteran", "Veteran", "m", "Gruff, deep, battle-worn", "veteran",
     "A gruff, deep male voice of a battle-worn veteran soldier in his fifties, gravelly and rough, "
     "low pitch, a bit hoarse, grumpy and world-weary.", 3),
    ("coach", "Coach", "f", "Cheerful, warm, encouraging", "coach",
     "A cheerful, energetic young woman in her late twenties, a friendly sports coach, warm "
     "mid-high pitch, bright and encouraging, clear articulation.", 3),
    ("expert", "Expert", "f", "Calm, precise, composed", "expert",
     "A calm, precise woman in her forties, a composed scientist, smooth medium-low pitch, "
     "measured pace, clear and confident, unhurried.", 1),
    ("joker", "Joker", "m", "Cocky, cheeky, a grin in his voice", "joker",
     "A cocky young man in his twenties, a cheeky prankster, playful and smug, medium pitch, "
     "quick and bouncy delivery, a grin in his voice.", 2),
    ("quiet", "Quiet one", "f", "Soft-spoken, gentle, a little shy", "quiet",
     "A soft-spoken shy young woman in her twenties, gentle and airy, light breathy voice, slow "
     "and careful, slightly timid.", 1),
    ("captain", "Old captain", "m", "Raspy old sea dog", None,
     "A raspy old sea captain in his sixties, a pirate-like rough gravelly voice, heavy and husky, "
     "rolling and theatrical, a weathered growl.", 3),
    ("speedrunner", "Speedrunner", "m", "Hyped, fast-talking streamer", None,
     "An energetic young male video game streamer around twenty, fast-talking and hyped, high "
     "energy, bright tenor, excited and punchy.", 1),
    ("nana", "Nana", "f", "Warm, kind older woman", None,
     "A warm, kind older woman in her late sixties, a gentle grandmother, soft and motherly, "
     "slightly low and mellow, reassuring and affectionate.", 2),
    ("teen", "Teen", "f", "Sarcastic, bored, unimpressed", None,
     "A sarcastic teenage girl around sixteen, bored and unimpressed, slightly nasal mid-high "
     "pitch, eye-rolling drawl, deadpan.", 1),
    ("knight", "Knight", "m", "Deep, stoic, noble", None,
     "A deep, stoic male knight in his forties, a resonant bass voice, solemn and steady, slow "
     "and noble, firm and controlled.", 1),
    ("sunny", "Sunny", "f", "Bright, bubbly, upbeat", None,
     "A bright, upbeat young woman around twenty, bubbly and sparkly, high pitch, giggly and "
     "enthusiastic, fast and lively.", 1),
    ("butler", "Butler", "m", "Dry, refined, British", None,
     "A dry, refined older British butler in his fifties, crisp received pronunciation, polite "
     "and understated, medium-low pitch, impeccably composed.", 1),
]

EMOTIONS = ["neutral", "shout", "whisper", "laugh", "sarcastic"]

# Appended to the character description for VoiceDesign.
DESIGN_EMOTION = {
    "neutral": "Speaking in a relaxed, natural conversational tone.",
    "shout": "Shouting at the top of the lungs, urgent and alarmed, loud and intense.",
    "whisper": "Whispering very quietly and breathily, tense and secretive.",
    "laugh": "Laughing heartily between words, genuinely amused.",
    "sarcastic": "Dry and sarcastic, deadpan, slightly mocking, exasperated.",
}

# VoxCPM2 controllable-clone style prefix.
VOX_STYLE = {
    "shout": "(shouting at the top of the lungs, urgent and alarmed)",
    "whisper": "(whispering very quietly and breathily, tense)",
    "laugh": "(laughing heartily, genuinely amused)",
    "sarcastic": "(dry, sarcastic and deadpan, mocking)",
}

# Reference clip transcripts (5-8 s each).
REF_TEXT = {
    "en": {
        "neutral": "Alright, I've got the torches. Let's head down the mine and see what we can find today.",
        "shout": "Watch out! There's a whole horde coming over the hill! Get behind the wall, right now, go!",
        "whisper": "Quiet... I can hear something moving in the dark. Stay close to me and don't make a sound.",
        "laugh": "Ha ha ha! Did you see that? The pig just jumped off the cliff and landed right in the water!",
        "sarcastic": "Oh, wonderful. Another hole full of gravel. This is exactly how I wanted to spend my evening.",
    },
    "fr": {
        "neutral": "Bon, j'ai les torches. On descend dans la mine et on va voir ce qu'on trouve aujourd'hui.",
        "shout": "Attention ! Il y a toute une horde qui arrive par la colline ! Derrière le mur, tout de suite, vite !",
        "whisper": "Doucement... j'entends quelque chose bouger dans le noir. Reste près de moi et ne fais pas de bruit.",
        "laugh": "Ha ha ha ! T'as vu ça ? Le cochon a sauté de la falaise et il est tombé en plein dans l'eau !",
        "sarcastic": "Oh, génial. Encore un trou rempli de gravier. C'est exactement comme ça que je voulais passer ma soirée.",
    },
}

# The sampler: (key, tone, en, fr)
SAMPLES = [
    ("calm", "neutral", "Nice work. Let's set up camp here and sort out the chests before night falls.",
     "Beau travail. On installe le camp ici et on range les coffres avant la nuit."),
    ("shout", "shout", "Creeper behind you! Move!", "Un creeper derrière toi ! Bouge !"),
    ("whisper", "whisper", "Shh... there's a skeleton below us.",
     "Chut... il y a un squelette juste en dessous de nous."),
    ("sarcastic", "sarcastic", "Oh great, you fell in lava. Again.",
     "Oh super, t'es tombé dans la lave. Encore."),
    ("salty", "shout", "What the hell are you doing?! Get back here!",
     "Mais qu'est-ce que tu fous ?! Reviens ici !"),
]

# The runtime consistency test: 10 lines per character, mixed tones and languages.
RUNTIME_LINES = [
    ("en", "neutral", "I found some iron over here. Want me to mine it?"),
    ("en", "shout", "Zombies! Three of them, on the left!"),
    ("en", "whisper", "Careful, there's a creeper right around that corner."),
    ("en", "laugh", "Ha! You just walked straight into the cactus."),
    ("en", "sarcastic", "Sure, let's dig straight down. What could possibly go wrong?"),
    ("en", "neutral", "It's getting dark. We should head back to the base."),
    ("fr", "neutral", "J'ai trouvé du fer par ici. Je le mine ?"),
    ("fr", "shout", "Des zombies ! Trois, sur la gauche !"),
    ("fr", "whisper", "Attention, il y a un creeper juste derrière ce coin."),
    ("fr", "sarcastic", "Bien sûr, creusons tout droit vers le bas. Qu'est-ce qui pourrait mal tourner ?"),
]
