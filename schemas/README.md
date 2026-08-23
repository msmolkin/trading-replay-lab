# Contract schemas

`schemas/v1/` is the canonical source for wire contracts. Generated language models under `packages/contracts/generated/` are derived artifacts and must not be edited by hand.

All 64-bit prices, quantities, money, rates, timestamps, sequences, and counters use canonical base-10 strings on JSON wires. This guarantees that JavaScript never silently rounds an integer. Generated domain models convert those strings to `bigint`, Python `int`, or Rust integer types after explicit range validation.

The v1 wire version is `1.0.0`. Readers reject unsupported major versions. Adding an optional field is minor-compatible; changing a unit, required field, enum meaning, rounding rule, or event ordering is breaking.
