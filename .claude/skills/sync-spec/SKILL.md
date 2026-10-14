---
name: sync-spec
description: Check and synchronize specification documents (docs-spec/) with implementation (src/, src-tauri/, tests/). Detect discrepancies and auto-fix when possible.
allowed-tools: Read, Glob, Grep, Edit, Write, Bash(cargo:*), AskUserQuestion
---

# Specification Synchronizer

## Instructions

### Step 1: Scan Specifications

1. Read all API specs from `docs-spec/api/*.md`
2. Read architecture from `docs-spec/architecture.md`
3. Read BDD scenarios from `docs-spec/behavior/*.feature`

For each spec file, extract:
- Rust code blocks containing `struct`, `enum`, `trait`, `fn` definitions
- Thread constraints (スレッド: リアルタイム/非リアルタイム/任意)
- Module structure diagrams

### Step 2: Scan Implementation

1. Find all Rust files: `src/**/*.rs`, `src-tauri/**/*.rs`
2. For each file, search for:
   - `pub struct` definitions
   - `pub enum` definitions
   - `pub trait` definitions
   - `pub fn` definitions
   - `impl` blocks

3. Find existing tests: `tests/**/*.rs`

### Step 3: Match and Compare

Use this mapping table:

| Spec File | Implementation Files |
|-----------|---------------------|
| docs-spec/api/audio_engine.md | src/audio/engine.rs, src/audio/device.rs, src/audio/effects.rs, src/audio/recording.rs, src/audio/metronome.rs |
| docs-spec/api/network.md | src/network/connection.rs, src/network/roster.rs, src/network/udp_mux.rs, src/network/fec.rs, src/network/transport.rs |
| docs-spec/api/signaling.md | src/network/signaling.rs |
| docs-spec/api/plugin.md | src/audio/plugin.rs |
| docs-spec/api/i18n.md | ui/locales/*.json, ui/src/i18n/* |
| docs-spec/behavior/*.feature | tests/*.rs, ui/src/**/*.test.ts(x) |
| docs-spec/requirements.md | src/audio/preset.rs, tests/*.rs |

Compare:
- Struct names and fields (name, type)
- Enum variants
- Trait methods (signature)
- Function signatures (name, parameters, return type)

### Step 4: Auto-fix or Ask

#### Auto-fix (high confidence):
- Missing struct/enum/fn in impl → Add stub to implementation
- Missing pub item in spec → Add to spec documentation
- Missing doc comments → Add based on spec description
- Missing test file → Create test skeleton

#### Ask user (low confidence):
- Signature differs (design choice needed)
- Implementation more complex than spec (generics, extra params)
- Conflicts with ADR decisions

Use AskUserQuestion when unsure (with recommendation):
- Always provide a recommended option based on project requirements
- Explain why the recommendation is best

Example:
- Question: "Signature mismatch for fn start_capture. Which to fix?"
- Options:
  1. "Update implementation (Recommended): Spec is source of truth per .claude/rules/spec-sync.md"
  2. "Update spec: If implementation has better design rationale"
  3. "Skip: Defer decision"

### Step 5: Ensure Tests

1. For each `Scenario:` in `docs-spec/behavior/*.feature`:
   - Map to test file: `connection.feature` → `tests/connection_test.rs`
   - If test file missing → Create with test skeleton
   - If scenario not covered → Add test function

2. Test function structure:
   - Parse `Given` → setup code
   - Parse `When` → action code
   - Parse `Then` → assertion

3. Run `cargo test` to verify

Never satisfy a scenario with an empty test body or an `assert!(true)`. If the
implementation does not exist yet, downgrade the scenario's tag from `@must` to
`@should`, note why in the `.feature` file, and let it appear as a gap in
`docs-spec/traceability.md` (see `.claude/rules/traceability.md`).

### Step 6: Verify Traceability

Run the traceability checks and act on what they report:

```bash
cargo test --test traceability_test
```

| Failure | Action |
|---------|--------|
| `requirement_definitions_are_well_formed` | Add the missing `@REQ-*` / `@must` / `@should` tag to the Scenario |
| `no_test_verifies_an_unknown_requirement` | Fix the typo, or define the requirement in `docs-spec/requirements.md` |
| `no_verifying_test_is_hollow` | Add real assertions, or remove the `Verifies:` claim |
| `every_must_requirement_is_verified` | Write the test, or downgrade to `@should` and record the gap |
| `traceability_matrix_is_up_to_date` | Regenerate: `JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test`, then review the diff |

Review the regenerated diff before committing: a `must` requirement that
silently became unverified, or a growing gap list, is a finding to report - not
something to accept quietly.

---

## Check Rules

### Type Definition Checks

| Check | Action if Failed |
|-------|-----------------|
| Struct in spec, missing in impl | Add struct stub to impl |
| Struct field type mismatch | ASK: Which is correct? |
| Enum variant missing | Add variant to impl |
| Trait method missing | Add method stub to impl |

### Function Signature Checks

| Check | Action if Failed |
|-------|-----------------|
| Function missing in impl | Add function stub |
| Parameter count differs | ASK: Design decision |
| Return type differs | ASK: Design decision |
| Thread constraint undocumented | Add doc comment |

### Module Structure Checks

| Check | Action if Failed |
|-------|-----------------|
| Module file missing | ASK: Create or update spec? |
| File exists but not in spec | Add to spec module diagram |

### Test Coverage Checks

| Check | Action if Failed |
|-------|-----------------|
| No test for scenario | Create test file and function |
| Test exists but incomplete | Add missing assertions |

---

## Output Format

```
## Sync Report: [spec_file] <-> [impl_files]

### Auto-fixed
- [FIXED] description (file:line)

### Needs Confirmation
- [QUESTION] description
  - Spec: X
  - Impl: Y

### Tests Created
- [TEST] tests/xxx_test.rs - N scenarios covered

### Traceability
- Requirements: N total (N must / N should)
- Verified: N
- Gaps (should, unverified): N
- [GAP] REQ-XXX-NNN - reason

### Summary
- Matched: N items
- Auto-fixed: N items
- User decisions: N items
- Tests created: N files
- Traceability matrix: up to date / regenerated
```
