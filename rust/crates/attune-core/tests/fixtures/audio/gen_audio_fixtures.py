#!/usr/bin/env python3
"""Deterministic audio fixture generator for ASR routing/quality tests.

Produces three committed WAV fixtures + one corrupt fixture used by
`tests/asr_ingest_test.rs`:

  tone_440hz.wav   — 1.0s 440 Hz sine, 16 kHz mono PCM16. Pure tone, no speech.
                     Routing fixture: proves `.wav` enters the ASR branch
                     regardless of whisper output (a real model returns empty /
                     no-speech text on a tone).
  silence.wav      — 0.5s of digital silence, 16 kHz mono PCM16. Edge fixture.
  speech_known.wav — synthesized speech of a KNOWN sentence via espeak, used by
                     the env-gated real-ASR leg to measure CER. Generated only
                     if `espeak` is on PATH; the committed copy is produced once
                     and checked in so CI is reproducible without espeak.
  corrupt.wav      — NOT a real WAV: 64 bytes of 0xFF with a .wav extension.
                     Graceful-error fixture (parser must route to ASR then Err,
                     not treat as text / not panic).

All PCM fixtures are byte-deterministic: stdlib `wave` + fixed sample math, no
RNG, no timestamps. Re-running yields identical bytes (verify via sha256).

The KNOWN sentence (must match KNOWN_TRANSCRIPT in asr_ingest_test.rs):
  "the quick brown fox jumps over the lazy dog"

Usage:
  python3 gen_audio_fixtures.py          # regenerate tone/silence/corrupt (+ speech if espeak)
  python3 gen_audio_fixtures.py --check   # print sha256 of each fixture
"""
import math
import os
import struct
import subprocess
import sys
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
SAMPLE_RATE = 16000  # whisper.cpp resamples internally; 16k keeps fixtures tiny
KNOWN_SENTENCE = "the quick brown fox jumps over the lazy dog"


def write_pcm16_wav(path, samples):
    """Write mono 16-bit PCM at SAMPLE_RATE. `samples`: iterable of int16."""
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(b"".join(struct.pack("<h", s) for s in samples))


def gen_tone(path, freq=440.0, seconds=1.0, amplitude=0.3):
    n = int(SAMPLE_RATE * seconds)
    peak = int(amplitude * 32767)
    samples = [
        int(peak * math.sin(2.0 * math.pi * freq * (i / SAMPLE_RATE)))
        for i in range(n)
    ]
    write_pcm16_wav(path, samples)


def gen_silence(path, seconds=0.5):
    n = int(SAMPLE_RATE * seconds)
    write_pcm16_wav(path, [0] * n)


def gen_corrupt(path):
    # Deliberately NOT a RIFF/WAVE file — exercises the graceful-Err path.
    with open(path, "wb") as f:
        f.write(b"\xff" * 64)


def gen_speech_espeak(path, sentence):
    """Best-effort known-content speech via espeak. Returns True on success."""
    if not _which("espeak"):
        return False
    # espeak -> 22050 Hz wav; whisper accepts it. Deterministic for a fixed
    # voice/speed/text, but espeak versions can differ — so we commit the output
    # and only regenerate when explicitly asked.
    try:
        subprocess.run(
            ["espeak", "-v", "en", "-s", "150", "-w", path, sentence],
            check=True,
            capture_output=True,
        )
        return os.path.exists(path) and os.path.getsize(path) > 44
    except Exception as e:  # noqa: BLE001 — fixture gen, surface and skip
        print(f"espeak failed: {e}", file=sys.stderr)
        return False


def _which(name):
    for p in os.environ.get("PATH", "").split(os.pathsep):
        cand = os.path.join(p, name)
        if os.path.isfile(cand) and os.access(cand, os.X_OK):
            return cand
    return None


def sha256(path):
    import hashlib

    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    check = "--check" in sys.argv
    fixtures = {
        "tone_440hz.wav": lambda p: gen_tone(p),
        "silence.wav": lambda p: gen_silence(p),
        "corrupt.wav": lambda p: gen_corrupt(p),
    }
    if not check:
        for name, fn in fixtures.items():
            p = os.path.join(HERE, name)
            fn(p)
            print(f"wrote {name} ({os.path.getsize(p)} bytes)")
        sp = os.path.join(HERE, "speech_known.wav")
        if gen_speech_espeak(sp, KNOWN_SENTENCE):
            print(f"wrote speech_known.wav ({os.path.getsize(sp)} bytes) "
                  f"via espeak: {KNOWN_SENTENCE!r}")
        else:
            print("speech_known.wav: espeak unavailable — leave committed copy")

    for name in list(fixtures) + ["speech_known.wav"]:
        p = os.path.join(HERE, name)
        if os.path.exists(p):
            print(f"sha256 {name}: {sha256(p)}")


if __name__ == "__main__":
    main()
