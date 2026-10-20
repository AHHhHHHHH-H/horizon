#!/bin/bash
# Download each manifest line (path size sha256) at the pinned audio.cpp-gguf revision, verify.
REV=02f5de92a82339ebb722ea6fdadc14b7bec43a7f
cd ~/builds/horizon-companion-scratch/voices-dl
while read -r p size sha; do
  [ -z "$p" ] && continue
  mkdir -p "$(dirname "$p")"
  if [ -f "$p" ] && [ "$(stat -f %z "$p")" = "$size" ]; then echo "have $p"; else
    curl -sSfL --retry 5 -C - -o "$p" "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/$REV/$p" || echo "FAIL dl $p"
  fi
  got=$(shasum -a 256 "$p" | cut -d' ' -f1)
  [ "$got" = "$sha" ] && echo "ok $p" || echo "BADSHA $p $got"
done < manifest.txt
echo ALLDONE
