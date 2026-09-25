# The demo circuit

A rougher flotation circuit with a scavenger recycle. It is the flowsheet in `flowsheet::demo`, the one
`crates/flowsheet/examples/recycle_circuit.rs` solves and `crates/flowsheet-cli/tests/fixtures/recycle.json` saves,
and the one most of the test suite reasons about.

## Topology

```
  Feed                     (source, 0 in / 1 out)
    │ S0
    ▼
  Mixer  ◄─────────────┐   (2 in / 1 out)
    │ S1               │
    ▼                  │ S4   recycle ← tear stream
  Flotation cell       │
    ├── S2 ──▶ Concentrate   (1 in / 2 out)
    │ S3               │
    ▼                  │
  Splitter ────────────┘   (1 in / 2 out)
    │ S5
    ▼
  Tailings                 (sink, 1 in / 0 out)
```

The Feed → Mixer → Flotation → Splitter → Mixer cycle is why a topological sort alone can't solve this: S4 must be
guessed and iterated to convergence.

## Species

| Id | Species     | Role                               |
|----|-------------|------------------------------------|
| 0  | `CuFeS2(s)` | chalcopyrite - the valuable mineral |
| 1  | `SiO2(s)`   | gangue - the worthless rock         |
| 2  | `H2O(l)`    | water                              |

All flows are mass flow rates in t/h. Temperature is solved by the energy balance, but every stream enters at 25 °C,
so every stream leaves at 25 °C as well.

## Unit models

| Unit        | Model                                                                                   |
|-------------|-----------------------------------------------------------------------------------------|
| Feed        | outlet = `[40.0, 360.0, 600.0]`                                                         |
| Mixer       | `outlet[i] = sum of inlet[i]`; outlet temperature is whatever makes `H(outlet) = sum H(inlets)` |
| Flotation   | recovery `r = [0.85, 0.05, 0.30]`; `concentrate[i] = inlet[i] * r[i]`, `tails[i] = inlet[i] * (1 - r[i])` |
| Concentrate | sink                                                                                    |
| Splitter    | recycle fraction `f = 0.3`; `recycle[i] = inlet[i] * f`, `tailings[i] = inlet[i] * (1 - f)` |
| Tailings    | sink                                                                                    |

The flotation cell is what makes this a plant rather than a plumbing diagram. A splitter sends the same composition
to both outlets, so no arrangement of splitters can concentrate anything; recovering each species at its own rate is
what separation means.

## Converged solution

Each species travels the loop independently, so one scalar equation per species is the whole answer. With `F` the
feed, `r` the recovery and `f` the recycle fraction, the mixer outlet `M` satisfies `M = F + f(1 - r)M`, so

```
M = F / (1 - f(1 - r))
```

and everything else follows. `flowsheet::demo::balance` is that formula, and the balance tests assert against it
rather than against the numbers below.

| Stream           | CuFeS2 | SiO2    | H2O     | Total    |
|------------------|-------:|--------:|--------:|---------:|
| S0 feed          | 40.000 | 360.000 | 600.000 | 1000.000 |
| S1 mixer out     | 41.885 | 503.497 | 759.494 | 1304.875 |
| S2 concentrate   | 35.602 |  25.175 | 227.848 |  288.625 |
| S3 cell tails    |  6.283 | 478.322 | 531.646 | 1016.250 |
| S4 recycle       |  1.885 | 143.497 | 159.494 |  304.875 |
| S5 tailings      |  4.398 | 334.825 | 372.152 |  711.375 |

Three properties the tests check:

1. **S2 + S5 = S0, per species**, to within the solver tolerance. Nothing accumulates at steady state, so whatever
   enters the plant must leave by one of the two exits; the recycle only inflates the internal flows.
2. **The concentrate is upgraded**: 4.00% CuFeS2 in the feed becomes 12.33% in S2, and the tailings drop to 0.62%.
3. **Amplification is per species**: `1 / (1 - f(1 - r))` is 1.047 for the chalcopyrite that mostly floats out, but
   1.399 for the gangue that keeps going round. No single number describes the circuit.

## Convergence

Starting from S4 = 0 with direct substitution, the error in each species shrinks by `f(1 - r)` each pass, so the
slowest species sets the rate. Here that is the gangue at 0.3 × 0.95 = 0.285, giving 17 passes to 1e-9. At `f = 0.9`
the factor is 0.855 and it takes 119, which is the case for Wegstein acceleration: it takes 11.
