#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "lameenc", "numpy"]
# ///
"""Encodes the owner's recordings in `assets/sounds/*.wav` into the MP3s the client ships in
`src/client/src/sounds/`: mixed down to mono, trimmed of the silence before the sound starts and
after its tail dies away, faded out over the last few milliseconds so the cut end does not
click, and peak-normalized just under full scale. The client scales each to its own playback
loudness, so the files carry no loudness of their own.

MP3 is the one compressed format every target browser decodes through Web Audio's
`decodeAudioData`: Chrome, Edge and WebView2, Firefox, and Safari.

    uv run --locked --script tools/encode-sounds.py

Run `uv lock --script tools/encode-sounds.py` after changing the dependencies above.
"""

import wave
from pathlib import Path

import click
import lameenc
import numpy as np

__all__ = ["encode_sounds"]

_TOP_DIR = Path(__file__).resolve().parent.parent
_SOURCE_DIR = _TOP_DIR / "assets" / "sounds"
_OUT_DIR = _TOP_DIR / "src" / "client" / "src" / "sounds"
_BIT_RATE_KBPS = 128
# LAME's slowest, best-sounding search, which costs nothing on clips this short.
_LAME_QUALITY = 2
_PEAK = 10 ** (-1 / 20)
_WINDOW_S = 0.005
# A window this far under the peak counts as silence at the start (the room before the sound) and the end (its tail died away).
_START_BELOW_PEAK_DB = 45
_END_BELOW_PEAK_DB = 60
_PRE_ROLL_S = 0.005
_FADE_OUT_S = 0.02
_SAMPLE_WIDTH_BYTES = 2


def _read_wav(path: Path) -> tuple[np.ndarray, int]:
    """Returns `path`'s 16-bit PCM samples mixed down to mono, in [-1, 1], and its rate."""
    with wave.open(str(path)) as wav:
        if wav.getsampwidth() != _SAMPLE_WIDTH_BYTES:
            raise click.ClickException(f"{path}: {wav.getsampwidth() * 8}-bit samples, expected 16-bit PCM")
        frames = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2").reshape(-1, wav.getnchannels())
        return frames.mean(axis=1) / 32768, wav.getframerate()


def _trim(samples: np.ndarray, rate_hz: int) -> np.ndarray:
    """Cuts `samples` to the windows loud enough to hear, with a short pre-roll and a fade-out at the end."""
    window_len = round(_WINDOW_S * rate_hz)
    windows = samples[: len(samples) // window_len * window_len].reshape(-1, window_len)
    rms_db = 20 * np.log10(np.sqrt((windows**2).mean(axis=1)) + 1e-12)
    peak_db = 20 * np.log10(np.abs(samples).max())
    start_idx = int(np.argmax(rms_db > peak_db - _START_BELOW_PEAK_DB)) * window_len
    end_idx = (int(np.nonzero(rms_db > peak_db - _END_BELOW_PEAK_DB)[0][-1]) + 1) * window_len
    trimmed = samples[max(start_idx - round(_PRE_ROLL_S * rate_hz), 0) : end_idx].copy()
    fade_len = round(_FADE_OUT_S * rate_hz)
    trimmed[-fade_len:] *= np.linspace(1, 0, fade_len)
    return trimmed


def _encode_mp3(samples: np.ndarray, rate_hz: int) -> bytes:
    encoder = lameenc.Encoder()
    encoder.set_channels(1)
    encoder.set_in_sample_rate(rate_hz)
    encoder.set_bit_rate(_BIT_RATE_KBPS)
    encoder.set_quality(_LAME_QUALITY)
    pcm = np.round(samples * 32767).astype("<i2").tobytes()
    return bytes(encoder.encode(pcm) + encoder.flush())


@click.command()
def encode_sounds() -> None:
    """Encodes every recording in assets/sounds/ to an MP3 in src/client/src/sounds/."""
    sources = sorted(_SOURCE_DIR.glob("*.wav"))
    if not sources:
        raise click.ClickException(f"no recordings in {_SOURCE_DIR}")
    _OUT_DIR.mkdir(parents=True, exist_ok=True)
    for source in sources:
        samples, rate_hz = _read_wav(source)
        trimmed = _trim(samples, rate_hz)
        encoded = _encode_mp3(trimmed * (_PEAK / np.abs(trimmed).max()), rate_hz)
        out = _OUT_DIR / f"{source.stem}.mp3"
        out.write_bytes(encoded)
        click.echo(f"{out.name}: {len(trimmed) / rate_hz:.3f} s of {len(samples) / rate_hz:.3f} s, {rate_hz} Hz, {len(encoded)} bytes")


if __name__ == "__main__":
    encode_sounds()
