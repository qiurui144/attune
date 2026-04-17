# Test Corpora

Real GitHub knowledge repositories used as test corpora. Managed by `scripts/download-corpora.sh`.

## Layout

```
rust/tests/corpora/
├── README.md          (this file, committed)
├── .gitignore         (ignores cloned repos)
├── rust-book/         (cloned at pinned tag; gitignored)
├── cs-notes/          (cloned at pinned commit; gitignored)
└── openai-cookbook/   (cloned at pinned tag; gitignored)
```

## Why Real Repos?

Random or synthetic test data fails to catch real-world issues:

- Character encoding edge cases
- Mixed Chinese/English tokenization
- Code-block vs prose chunking boundaries
- Truly relevant vs "somewhat relevant" ranking

## Version Pinning

Each corpus is pinned to a specific tag or commit in `scripts/download-corpora.sh`.
Changing the pin is a deliberate test-quality decision — commit the change
with a note on expected quality metric shifts.

## Download

```bash
./scripts/download-corpora.sh            # all
./scripts/download-corpora.sh rust-book  # one
```

Corpora are NOT committed to the main repo. Each developer / CI runs the
download script. Total size ~200 MB.
