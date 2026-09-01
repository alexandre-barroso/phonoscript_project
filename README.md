# Project PhonoScript

Project PhonoScript is the single repository for PhonoScript and ConvalGEN.

- **PhonoScript** is the standalone scripting language and shared engine for
  finite constraint-based phonological analysis.
- **ConvalGEN** is the desktop application built on that engine. It provides
  visual project editing, tableaux, diagnostics, plots, and publication
  export.

The dependency is one-way: ConvalGEN uses the `phonoscript` crate;
PhonoScript has no dependency on the graphical application. Both components
are version 1.1.0. PhonoScript language version 3 and `.ottab` schema version 4
are separate compatibility contracts.

## Manuals

- [PhonoScript Language and Engine Manual](docs/PhonoScript-Language-Manual.pdf)
- [ConvalGEN User Guide](docs/ConvalGEN-User-Guide.pdf)

The corresponding self-contained LaTeX sources are stored beside the PDFs in
`docs/`.

Rebuild both manuals with a TeX distribution that provides XeLaTeX,
`latexmk`, Source Sans, and Source Code Pro:

```bash
latexmk -xelatex -interaction=nonstopmode docs/PhonoScript-Language-Manual.tex
latexmk -xelatex -interaction=nonstopmode docs/ConvalGEN-User-Guide.tex
latexmk -c docs/PhonoScript-Language-Manual.tex
latexmk -c docs/ConvalGEN-User-Guide.tex
```

## Repository layout

| Path | Contents |
|---|---|
| `phonoscript/` | Language frontend, interpreter, engine, fixtures, tests, standalone packaging, and the reference-bounded analysis corpus |
| `convalgen/` | Desktop interface, native packaging, dissertation project, bundled tableau style, screenshots, and export fixtures |
| `docs/` | The two public manuals and their LaTeX sources |
| `Cargo.toml` | Shared Rust workspace definition |

## Analysis contract

The shared engine evaluates declared finite analyses in strict Optimality
Theory, Harmonic Grammar, finite Maximum Entropy grammar, and serial OT/HG. It
also implements typed Second-Order comparisons and Q-Calculus operations.

Every violation count is supplied by the phonologist. Candidate forms,
structured representations, constraint names, and descriptive definitions do
not generate or replace marks. A new GUI cell is unset (`—`), not zero, and
evaluation, learning, comparison, and export are refused until the phonologist
completes the ledger. PhonoScript candidate imports likewise require explicit
violation vectors.

Candidate and output are not synonyms. Every tableau row is a candidate; the
term **output** applies only to a candidate selected by an evaluator or to a
declared serial terminal result.

Second-Order comparison calculates source and target responses independently
before applying a declared transport. Each side uses its tableau-level
evaluator and temperature when present, otherwise the project defaults; these
overrides therefore affect the calculated responses rather than only their
display. Its result is one of:

- `PRESERVED`, with a certificate;
- `DISCREPANCY`, with every differing response coordinate; or
- `NOT EVALUATED`, with the missing formation or admission dependency.

A missing support, transport, normalizer policy, scientific-layer bridge,
serial ledger, or violation count is never converted into `false`, zero,
`NaN`, or an empty result.

Exact rational source values remain exact in PhonoScript and `.ottab`.
Weighted HG and MaxEnt candidate costs remain exact when every enabled weight
is exact. Strict OT has no scalar Harmony: `evaluate()` reports `null` for that
row coordinate, and `harmony(candidate)` returns a structured formation fault.
MaxEnt exponentials and probabilities, numerical learning, and weighted costs
that contain an explicitly approximate weight cross a declared approximate
boundary. Exact finite MaxEnt-law comparison is a separate symbolic operation;
tolerance and finite-grid judgments are separate modes selected by the analyst.

Q-Calculus has registered semantics for strict OT with retained
co-winner sets, no project-level a-priori ranking relations, exactly aligned
finite constraint registers, 1--60 enabled constraints, and at most 63
candidates per tableau. Counts and reduced ranking shares are exact
arbitrary-precision integers and rationals. The default dynamic-program budget
is 2,000,000 charged states; reaching it is a structured refusal, not an
approximate count. A ranking share is not a token probability unless an
additional measure and response law are declared.

## Build and run

Rust 1.88 or newer is required for source builds. From the repository root:

```bash
cargo run --release -p convalgen

cargo run --release -p phonoscript -- \
  phonoscript/validation/analyses/published/kager-coda-voicing.phont
```

Install the language interpreter from source:

```bash
cargo install --locked --path phonoscript --bin phonoscript
phonoscript analysis.phont
phonoscript --check analysis.phont
```

PhonoScript 3 supports selective imports from local `.phont` modules:

```phont
import { build_analysis, title as analysis_title } from "./analysis.phont"

print(analysis_title)
build_analysis()
evaluate()
```

Imported names are immutable and only explicitly exported bindings are
visible. Imported files are declaration-only: they may contain imports,
functions, and immutable side-effect-free declarations, while project
mutation, output, and file effects must occur inside an exported function
called explicitly by the entry script. Relative imports are canonicalized and
confined beneath an explicit module root; symlink escapes, cycles, excessive
depth, excessive source volume, and missing exports produce source-located
diagnostics. There is no remote loading or package registry. The `--base`
option supplies project data, not executable declarations.

Run a module entry with an explicit root:

```bash
phonoscript --module-root analyses analyses/main.phont
phonoscript --check --module-root analyses analyses/main.phont
```

Run a script against an existing project and save the validated result:

```bash
phonoscript analysis.phont --base base.ottab --write result.ottab
```

The ConvalGEN editor opens and saves `.ottab` projects and runs `.phont`
scripts through the same parser, runtime, and engine as the command-line
interpreter.

## Native packages

Standalone PhonoScript packages are stored by platform under
`phonoscript/compiled/`. ConvalGEN packages are stored under
`convalgen/compiled/` and include the PhonoScript interpreter.

Build on the matching operating system:

```bash
./phonoscript/scripts/package-macos.sh
./phonoscript/scripts/package-linux.sh

./convalgen/source/scripts/package-macos.sh
./convalgen/source/scripts/package-linux.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\phonoscript\scripts\package-windows.ps1
powershell -ExecutionPolicy Bypass -File .\convalgen\source\scripts\package-windows.ps1
```

A package name records its operating system and architecture. The macOS
archives are locally exercised. Linux and Windows archives produced by the
macOS cross-build gate are linked for their declared targets and inspected for
binary format and path hygiene, but they are not native execution evidence;
the matching native CI jobs remain the runtime gate. Public macOS distribution
also requires the release owner's Developer ID signature and notarization;
local packages are ad-hoc signed.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-targets
cargo run --release -p phonoscript --bin qcalc-bench
```

The corpus harness discovers and executes every checked-in authored `.phont`
program transactionally. The reference-project harness enumerates every
checked-in `.ottab` fixture, decodes and validates it, and evaluates its
declared oracle boundary. Cross-interface tests compare independent native
transcriptions, PhonoScript programs, and `.ottab` documents where all three
exist. The dissertation fixture is also checked against the copy distributed
with ConvalGEN. Discovery-based checks keep this documentation from becoming
incorrect whenever a validated asset is added.

The validation corpus distinguishes seven claim types:

- **source-exact finite evaluator replication** for the printed Kager
  final-voicing panels, Pater Tableau (13), and all four printed Anttila–Cho
  competition counts;
- **source-bounded normalized correction** for Tessier's printed `/skul/`
  HG/MaxEnt ledgers: the checked transcription preserves the printed support,
  weights, violations, and costs, corrects the printed decimal for
  `exp(-11)`, and supplies the missing finite MaxEnt normalizer;
- **source-exact ledger transcription with structured refusal** for the
  authored PhonoScript and `.ottab` versions of the Goldwater–Johnson Tables
  2–3 ledger, whose unpublished fitted weights prevent probability replay;
- **bounded reconstruction** for the McCarthy Rimi fragment and the
  stable-labelled dissertation transcription and comparison records;
- **synthetic or derivative checks** for generic engine and language behavior;
- **source-inspired checks**, including the partial Prince–Smolensky Berber
  fixture, that test a named mechanism without licensing source replication;
  and
- **refusal cases** when a publication or project omits a required dependency.

The Tessier correction is an engine-derived conditional law on the printed
finite support, not a claim that the source printed normalized probabilities,
a complete `GEN`, or learned weights. The Goldwater–Johnson fitted
probabilities are not replayed because the published analysis does not provide
the learned weight vector. The Kager basic-syllable, harmonic-bounding, and
tie/order scripts are small derivative checks, not complete textbook
transcriptions.

Each of the 39 bounded dissertation records carries a LaTeX source label shared
by the English and Portuguese versions, such as `fig:...` or `tab:...`, in
`source_locator`.
Its fidelity tag states the replay ceiling: `fidelity:exact`,
`fidelity:exact-transform`, `fidelity:neutral-ledger`,
`fidelity:snapshot-only`, `fidelity:flattened-panel`, or
`fidelity:source-panel-only`. Human-facing record names such as `H.1` aid
navigation but are not the provenance key. Passing the suite establishes only
the tagged finite records and software properties; it does not establish
exhaustive coverage of the reference literature, completeness of a
user-declared `GEN`, empirical warrant, or historical priority.

The executable grammar domain is finite. The project does not implement a
general continuous-HG candidate space, continuous optimizer, KKT/persistence
certificate, or contact/inversion solver. A Second-Order grid judgment compares
the declared finite sample points; it does not certify equality over a
continuous domain.

The fixed performance gate separately exercises exact ranking-space counting,
50,000 finite-MaxEnt evaluations, and 20,000 complete typed Second-Order
comparisons. Its budgets apply to the machine that runs it and are not a
universal hardware promise.

## Publication output and interface evidence

ConvalGEN exports cropped SVG, PDF, and PNG from one native vector scene. SVG
remains editable; PDF retains vector text and embedded fonts; PNG uses the
explicit numeric SVG canvas and declared scale. The renderer implements the
material visual grammar of the bundled `secondordertableau.sty` package and
never emits a `.tex` file.

The checked visual corpus covers strict OT, HG, MaxEnt, serial derivations and
refusals, preservation/discrepancy/refusal Second-Order layouts, Q-Calculus,
plot families, Unicode labels, and dense matrices. Publication tiling breaks
only between complete candidate rows and constraint or metric columns, and
repeats the input/header band, relevant constraint labels, and candidate
register on every continuation page. Live macOS inspection covered compact, standard, wide,
and portrait layouts, all workspaces, scrolling, preferences, help, about,
save/export dialogs, and structured error states. These checks do not claim
pixel identity on every platform or display configuration.

## Contributing and security

A bug report should state the version, operating system and architecture, the
smallest self-contained `.phont` or `.ottab` reproduction, the calculated
result, and the expected result with its mathematical or scholarly basis.
Remove confidential research data before sharing a file.

An evaluator change requires a manually checkable finite case, an automated
regression, a declared response type and exactness boundary, and a precise
source locator when the test reproduces published work. Missing candidates,
weights, normalizers, transports, or observation bridges must not be filled
with invented values.

Treat scripts and projects from untrusted sources as untrusted input. Review
explicit save and export destinations before execution. Report a suspected
vulnerability privately through <https://alexandrebarroso.com>.

## Release 1.1.0

- separated PhonoScript into an independent language and engine crate;
- introduced PhonoScript language version 3, safe local modules, and
  `.ottab` schema version 4;
- made unset violation cells first-class and kept every violation ledger under
  phonologist control;
- added structured phonology, declared finite `GEN`, transactional execution,
  typed refusals, exact finite-law comparison, and the unified analysis corpus;
- expanded ConvalGEN with direct table editing, multi-tableau projects,
  responsive scrolling, keyboard shortcuts, serial and Second-Order
  workspaces, Q-Calculus, plotting, help, preferences, and native publication
  export; and
- validated 39 stable-labelled, fidelity-tagged dissertation records and the
  bounded published-source cases listed above.

## License and citation

Project PhonoScript is free and open-source software under the MIT License.
Copyright © 2026 Alexandre Menezes Barroso.

Citation metadata is provided in `CITATION.cff`.
