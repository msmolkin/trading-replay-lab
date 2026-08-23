# Synthetic fixture generator

This generator creates the tiny F0/F1/F2 market streams under `fixtures/micro/`. They are wholly synthetic and CC0-1.0; no exchange or provider data is embedded.

Run `python tools/fixture-generator/generate.py` to regenerate and `python tools/fixture-generator/generate.py --check` to prove the committed bytes match the deterministic generator.

The fixture set deliberately contains equal timestamps with deterministic tie breakers, sequence gaps, partial L2 depth and resynchronization, an ambiguous OHLC bar, a funding event, and a corporate-action split.
