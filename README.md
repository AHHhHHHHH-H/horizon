# nightly-state

Written by tools/ci/nightly_set.py and tools/ci/nightly.py, never by hand. `set.json`: the pull requests the trusted
reviewers picked for the Nightly (docs/release-flow.md "Nightly channel"), each at the head it was asked at.
`builds.jsonl`: one line per Nightly build with its set.
