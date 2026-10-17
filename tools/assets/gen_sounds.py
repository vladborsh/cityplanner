#!/usr/bin/env python3
"""Generates the game's sound effects (assets/sounds/*.wav): self-made, synthesized from sines,
filtered noise and decaying envelopes with a fixed seed, so a run reproduces the committed
files byte for byte. Not part of the build; the WAVs are committed (CC0, see assets/LICENSES.md).

    python3 tools/assets/gen_sounds.py
"""
import math
import os
import random
import struct
import wave

RATE = 22050
OUT = os.path.join(os.path.dirname(__file__), "..", "..", "assets", "sounds")


def env(t, dur, attack=0.004, decay=6.0):
    """Fast attack, exponential decay, faded out to nothing at `dur`."""
    a = min(1.0, t / attack) if attack > 0 else 1.0
    fade = min(1.0, (dur - t) / 0.02)
    return a * math.exp(-decay * t) * max(0.0, fade)


def render(dur, fn):
    n = int(RATE * dur)
    return [fn(i / RATE) for i in range(n)]


def lowpass(xs, k):
    out, y = [], 0.0
    for x in xs:
        y += k * (x - y)
        out.append(y)
    return out


def write(name, xs, gain=0.8):
    peak = max(1e-9, max(abs(x) for x in xs))
    path = os.path.join(OUT, name + ".wav")
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(32767 * gain * x / peak)) for x in xs))


def tone(f, t, phase=0.0):
    return math.sin(2 * math.pi * f * t + phase)


def main():
    os.makedirs(OUT, exist_ok=True)
    rng = random.Random(189)

    # Asphalt thunk: a low thump with a short noise tick.
    noise = [rng.uniform(-1, 1) for _ in range(RATE)]
    n = lowpass(noise, 0.25)
    write("road_built", render(0.28, lambda t: env(t, 0.28, decay=16) * (tone(80 - 40 * t, t) + 0.5 * n[int(t * RATE)] * math.exp(-60 * t))))

    # Soft brush: filtered noise swish, quiet.
    n2 = lowpass([rng.uniform(-1, 1) for _ in range(RATE)], 0.12)
    write("zone_painted", render(0.2, lambda t: env(t, 0.2, attack=0.04, decay=9) * n2[int(t * RATE)]), gain=0.5)

    # Construction clank: inharmonic metal partials and a thump.
    partials = [(523, 1.0), (1131, 0.6), (1763, 0.4), (2411, 0.25)]
    write("building_placed", render(0.4, lambda t: env(t, 0.4, decay=9) * sum(a * tone(f, t) for f, a in partials) + 0.8 * env(t, 0.4, decay=30) * tone(70, t)))

    # Pipe / power line: a short metal click, two tones.
    write("utility_built", render(0.14, lambda t: env(t, 0.14, attack=0.001, decay=34) * (tone(2200, t) + 0.6 * tone(3300, t))))

    # Demolition crunch: noise bursts with crackle.
    n3 = [rng.uniform(-1, 1) for _ in range(RATE)]
    crack = [rng.random() < 0.002 for _ in range(RATE)]
    ln = lowpass(n3, 0.5)

    def crunch(t):
        i = int(t * RATE)
        return env(t, 0.5, attack=0.002, decay=7) * (ln[i] + (1.2 if crack[i] else 0.0) * n3[i]) + 0.7 * env(t, 0.5, decay=22) * tone(55, t)

    write("demolish", render(0.5, crunch))

    # Invalid action: a dull low buzz that drops.
    write("error", render(0.22, lambda t: env(t, 0.22, attack=0.005, decay=7) * (1 if math.sin(2 * math.pi * (150 - 80 * t) * t) > 0 else -1) * 0.6), gain=0.5)

    # UI click: a tiny tick.
    write("ui_click", render(0.04, lambda t: env(t, 0.04, attack=0.0005, decay=60) * tone(1500, t)), gain=0.4)

    # Advisor chimes by severity: info (rising pair), warning (two same notes), alert (falling triple).
    def chime(notes, step, dur):
        def f(t):
            s = 0.0
            for k, fr in enumerate(notes):
                u = t - k * step
                if u >= 0:
                    s += env(u, dur, attack=0.006, decay=5) * (tone(fr, u) + 0.3 * tone(2 * fr, u))
            return s

        return f

    write("advisor_info", render(0.55, chime([880, 1320], 0.12, 0.55)), gain=0.6)
    write("advisor_warn", render(0.6, chime([660, 660], 0.18, 0.6)), gain=0.7)
    write("advisor_alert", render(0.8, chime([880, 660, 440], 0.16, 0.8)), gain=0.8)


if __name__ == "__main__":
    main()
