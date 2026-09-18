# Flowsheet

A steady-state process simulation library and CLI tool which can be used to solve flowsheets.

There are two crates:

- **`flowsheet`**: the core library.
  - Build a flowsheet in code or load one from JSON, then solve it.
  - Extend the current set of available unit types.
- **`flowsheet-cli`**: the `flowsheet` CLI binary.
  - Define a JSON flowsheet, then solve it and prints the results.

## CLI Quickstart

```bash
cargo install flowsheet-cli
```

Define a flowsheet in JSON, then solve it:

```bash
flowsheet blend.json
```

Command line options:

| Option                   | Default  | What it does                                                                                   |
|--------------------------|----------|------------------------------------------------------------------------------------------------|
| `--json`                 | off      | Print the solved flowsheet as a JSON document instead of a table                               |
| `--tolerance <x>`        | `1e-9`   | How little the recycle streams may change between passes before the solve counts as finished  |
| `--max-iterations <n>`   | `100`    | How many passes to make before giving up with an error                                         |
| `--method <m>`           | `direct` | How each pass guesses the recycle streams: `direct` reuses the last result, `wegstein` extrapolates from the last two and needs far fewer passes on a heavy recycle |

## Worked example: heating a blend with a recycle

Cold water and ethanol are blended, heated, and half of the hot blend is sent back to the mixer. The rest leaves as
product.

```
water   ──►┐
           mixer ──► heater ──► split ──► product
ethanol ──►┘  ▲                   │
              └───────────────────┘
```

- Water enters at 90 t/h and 15 °C (288.15 K). Ethanol enters at 10 t/h and 25 °C.
- The heater adds 16,000 MJ/h.
- The splitter sends half its flow back to the mixer.

`blend.json`:

```json
{
  "species": [
    {
      "name": "H2O", "phase": "Liquid", "molar_mass": 18.015,
      "shomate": { "a": -203.606, "b": 1523.29, "c": -3196.413, "d": 2474.455, "e": 3.855326 }
    },
    { "name": "C2H5OH", "phase": "Liquid", "molar_mass": 46.07, "shomate": { "a": 112.4 } }
  ],
  "units": [
    {
      "name": "water",
      "op": { "type": "feed", "state": { "flows": { "H2O": 90.0 }, "temperature": 288.15 } }
    },
    {
      "name": "ethanol",
      "op": { "type": "feed", "state": { "flows": { "C2H5OH": 10.0 } } }
    },
    { "name": "mixer", "op": { "type": "mixer" } },
    { "name": "heater", "op": { "type": "heater", "duty": 16000.0 } },
    { "name": "split", "op": { "type": "splitter", "fraction": 0.5 } },
    { "name": "product", "op": { "type": "product" } }
  ],
  "streams": [
    { "from": "water", "to": "mixer" },
    { "from": "ethanol", "to": "mixer" },
    { "from": "mixer", "to": "heater" },
    { "from": "heater", "to": "split" },
    { "from": "split", "to": "mixer" },
    { "from": "split", "to": "product" }
  ]
}
```

```bash
flowsheet blend.json
```

```
  #  stream             H2O  C2H5OH    total   T (K)
  0  water.mixer     90.000   0.000   90.000  288.15
  1  ethanol.mixer    0.000  10.000   10.000  298.15
  2  mixer.heater   180.000  20.000  200.000  308.71
  3  heater.split   180.000  20.000  200.000  328.68
  4  split.mixer     90.000  10.000  100.000  328.68
  5  split.product   90.000  10.000  100.000  328.68
     converged in 30 iterations, residual 8.4e-10, imbalance 8.4e-10

  unit    type    duty (MJ/h)
  heater  heater      16000.0
```

Below the footer, every unit that takes heat in or gives it out is listed with its duty: its
outlets' enthalpy flow minus its inlets'. For a heater that is the duty it was given; for an
isothermal reactor it is the heat of reaction it had to shed; for a compressor it is its whole
shaft work, and for a pump only the part of its shaft work that became heat rather than pressure.
Units that only mix or split are left out.

## The JSON format

A document has three lists: `species`, `units`, `streams`.

### Units of measure

| Quantity      | Unit              |
|---------------|-------------------|
| Mass flow     | t/h, per species  |
| Temperature   | K                 |
| Pressure      | kPa               |
| Heater duty   | MJ/h              |
| Pressure drop, pressure rise | kPa |
| Molar mass    | g/mol             |
| Enthalpy of formation | kJ/mol    |
| Density       | kg/m³             |

### `species`

| Field        | Required | Notes                                                                                                                                                                    |
|--------------|----------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `name`       | yes      | Unique across the document.                                                                                                                                              |
| `phase`      | yes      | `Solid`, `Liquid` or `Gas`                                                                                                                                               |
| `molar_mass` | yes      | g/mol                                                                                                                                                                    |
| `shomate`    | yes      | Heat capacity, [NIST Shomate](https://webbook.nist.gov/chemistry/) `a`-`e`. Only `a` is required; omitted terms are zero, so `{ "a": 75.3 }` is a constant 75.3 J/(mol·K). |
| `enthalpy_of_formation` | no | Standard enthalpy of formation at 298.15 K, kJ/mol: NIST's `H`. Only needed by a species that a reaction creates or destroys. |
| `density`    | no       | kg/m³, held constant. Only needed by a species that flows through a `pump`.                                                                              |

### `units`

Each unit has a unique `name` and an `op`. The `op` has a `type` plus that operation's parameters.

| `type`       | Inlets | Outlets | Parameters                                                                     |
|--------------|--------|---------|--------------------------------------------------------------------------------|
| `feed`       | 0      | 1       | `state`: `flows` (species map, omitted = 0), `temperature` (default 298.15), `pressure` (default 101.325) |
| `mixer`      | 1+     | 1       | `pressure_drop`: kPa, optional, default 0. Outlet pressure is the lowest flowing inlet minus this |
| `splitter`   | 1      | 2       | `fraction`: share sent to the **first** outlet, 0 to 1. `pressure_drop`: kPa, optional |
| `splitter_n` | 1      | N       | `ratios`: one per outlet, relative, so `[3, 7]` is a 30/70 split. `pressure_drop`: kPa, optional |
| `flotation`  | 1      | 2       | `recovery`: species map, share of each to the concentrate (omitted = 0). Outlets: concentrate, then tails. `pressure_drop`: kPa, optional |
| `heater`     | 1      | 1       | `duty`: MJ/h, negative cools. `pressure_drop`: kPa, optional                  |
| `conversion_reactor` | 1 | 1   | `energy`: `isothermal` or `adiabatic`. `reactions`: a list, run in order, each with `stoichiometry` (species map of molar coefficients, negative consumed), `limiting` (a reactant's name) and `conversion` (0 to 1, a share of what reaches that reaction). `pressure_drop`: kPa, optional |
| `pump`       | 1      | 1       | `pressure_rise`: kPa, added to the stream as an incompressible fluid. `efficiency`: 0 to 1, optional, default 1; the inefficient share of `V * dP / efficiency` warms the stream. Every species that flows through needs a `density` |
| `compressor` | 1      | 1       | `pressure_rise`: kPa, added to the stream as an ideal gas along an isentropic path. `efficiency`: isentropic, 0 to 1, optional, default 1. Every species that flows through must be a `Gas` |
| `tank`       | 1      | 1       | none                                                                           |
| `product`    | 1      | 0       | none                                                                           |

### `streams`

```json
{ "from": "feed", "to": "mixer" }
```

A unit's outlets are assigned in the order its streams are declared - this matters for units with multiple outlets.
