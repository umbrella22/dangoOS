# RFC 0001 execution ledger

> Implementation status: **not started**. RFC acceptance and separate implementation authorization are pending.
> Authoring a complete plan does not complete W0 or any product stage.

Plan: [Configurable Rust Component Library](component-library-development-plan.md).

## Current handoff

- First pending package: **W0.1**.
- All six waves and every implementation package remain `not_started`.
- No build/test/provider/package/UI acceptance evidence was collected during authoring.
- The snapshot below includes the new plan/index and excludes only this ledger, according to plan section 14. It is an authoring snapshot, not a tested implementation revision.
- The local checkout is one README-only commit behind its observed tracking ref; no synchronization was performed. Recheck before execution.
- A pending authorization or design decision is not a newly discovered product defect.

## Machine-readable state

The JSON block follows the plan's ledger schema. Null command registrations are bounded discovery work, not passing gates.

```json
{
  "schema_version": 1,
  "plan_id": "lvgl-component-library-v1",
  "plan_revision": 1,
  "updated_at": "2026-10-02T18:00:55Z",
  "plan_status": "draft",
  "implementation_authorized": false,
  "checkout": {
    "revision": "5e3b9efab4d32ff514d2820354dcdef4bff857e2",
    "status_sha256": "d4e049d26957d3c832d17a282d81c0da2977915aab3199d61518e1184a183dc7",
    "relevant_diff_sha256": "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
    "relevant_untracked_sha256": "2e7f74f8cb8267faffa2ad0b45945096491813292263358bcf8fe7406916fa1c",
    "lockfile_sha256": "0cc84630f7d5dbcc717b6a9bdb2d5993687175927d425a8f535dcd0af00650ce",
    "fingerprint": "2c01b9c1dc86c07d192b84aa482891c7a9da6830a62ae3a8a748d030d993bd0d"
  },
  "current_wave": "W0",
  "current_work_package": "W0.1",
  "wave_state": "not_started",
  "wave_states": {
    "W0": "not_started",
    "W1": "not_started",
    "W2": "not_started",
    "W3": "not_started",
    "W4": "not_started",
    "W5": "not_started"
  },
  "package_states": {
    "W0.1": "not_started",
    "W0.2": "not_started",
    "W0.3": "not_started",
    "W1.1": "not_started",
    "W1.2": "not_started",
    "W1.3": "not_started",
    "W1.4": "not_started",
    "W2.1": "not_started",
    "W2.2": "not_started",
    "W2.3": "not_started",
    "W2.4": "not_started",
    "W3.1": "not_started",
    "W3.2": "not_started",
    "W3.3": "not_started",
    "W3.4": "not_started",
    "W4.1": "not_started",
    "W4.2": "not_started",
    "W4.3": "not_started",
    "W4.4": "not_started",
    "W5.1": "not_started",
    "W5.2": "not_started",
    "W5.3": "not_started"
  },
  "clean_acceptance_count": 0,
  "last_completed_action": null,
  "next_action": "After separate implementation authorization, execute W0.1: record RFC acceptance, recheck checkout/fingerprint and decide compatibility/default-font/release boundaries before any production change.",
  "required_gates": [
    {
      "id": "G-BASE",
      "status": "not_run",
      "invocation": "git status --short; git rev-parse HEAD; git diff --binary HEAD; git diff --cached --binary",
      "registration": null
    },
    {
      "id": "G-BUILD",
      "status": "not_run",
      "invocation": "cargo build --verbose",
      "registration": null
    },
    {
      "id": "G-TEST",
      "status": "not_run",
      "invocation": "cargo test --verbose -- --test-threads 1",
      "registration": null
    },
    {
      "id": "G-EXAMPLES",
      "status": "not_run",
      "invocation": "cargo check -p lvgl --examples --features alloc",
      "registration": null
    },
    {
      "id": "G-SDL",
      "status": "not_run",
      "invocation": "cargo check -p lvgl --features sdl --examples",
      "registration": null
    },
    {
      "id": "G-SYS",
      "status": "not_run",
      "invocation": "cargo test -p lvgl-sys --test build_script -- --test-threads=1",
      "registration": null
    },
    {
      "id": "G-CODEGEN",
      "status": "not_run",
      "invocation": "cargo test -p lvgl-codegen",
      "registration": null
    },
    {
      "id": "G-LVGL",
      "status": "not_run",
      "invocation": "cargo test -p lvgl -- --test-threads=1",
      "registration": null
    },
    {
      "id": "G-CORE",
      "status": "not_run",
      "invocation": "cargo check -p lvgl --no-default-features --lib",
      "registration": null
    },
    {
      "id": "G-NODEFAULT",
      "status": "not_run",
      "invocation": "cargo test -p lvgl --no-default-features -- --test-threads=1",
      "registration": null
    },
    {
      "id": "G-ALLOC",
      "status": "not_run",
      "invocation": "cargo test -p lvgl --no-default-features --features lvgl_alloc,rust_timer -- --test-threads=1",
      "registration": null
    },
    {
      "id": "G-FEATURES",
      "status": "not_run",
      "invocation": "cargo test -p lvgl --features alloc,sdl,demo-widgets,rust_timer,custom_timer -- --test-threads=1",
      "registration": null
    },
    {
      "id": "G-SIM",
      "status": "not_run",
      "invocation": "cargo run -p lvgl --example sdl_demo --features sdl",
      "registration": null
    },
    {
      "id": "G-PACKLIST",
      "status": "not_run",
      "invocation": "cargo package -p lvgl-sys --list --allow-dirty --offline --locked",
      "registration": null
    },
    {
      "id": "G-DIFF",
      "status": "not_run",
      "invocation": "git diff --check; git diff --name-only HEAD; git ls-files --others --exclude-standard",
      "registration": null
    },
    {
      "id": "D-CONFIG",
      "status": "not_run",
      "invocation": null,
      "registration": "W1.1 registers the exact fixture invocation for two configs, host/target split, custom font/C inputs, incremental mutation and interruption"
    },
    {
      "id": "D-FEATURE",
      "status": "not_run",
      "invocation": null,
      "registration": "W2.1 registers compile-pass/fail and dependency-union fixture commands after fixtures exist"
    },
    {
      "id": "D-SIZE",
      "status": "not_run",
      "invocation": null,
      "registration": "W2.1 records actual final-link, map, symbol and section tools/flags for the frozen Linux ELF fixture"
    },
    {
      "id": "D-PROVIDER",
      "status": "not_run",
      "invocation": null,
      "registration": "W3.1 registers actual external archive creation, receipt/probe generation, consumer link and negative-case commands"
    },
    {
      "id": "D-PACKAGE",
      "status": "not_run",
      "invocation": null,
      "registration": "W3.1 registers package creation, unpack, isolated dependency wiring, and consumer build/run commands for all three crates"
    },
    {
      "id": "D-LIFETIME",
      "status": "not_run",
      "invocation": null,
      "registration": "W4.1 registers native failure injection, compile-fail lifetime checks and repeated teardown commands"
    },
    {
      "id": "D-GALLERY",
      "status": "not_run",
      "invocation": null,
      "registration": "W4.1 reserves and then registers the real manifest example invocation and interaction scenario"
    },
    {
      "id": "D-STATIC",
      "status": "not_run",
      "invocation": null,
      "registration": "W0 inventories supported format/lint commands and legacy failures; registers exact touched-scope gate"
    },
    {
      "id": "D-DOC",
      "status": "not_run",
      "invocation": null,
      "registration": "W0 records the documentation link/fence/scope checker used for this plan and future migration docs"
    }
  ],
  "conditional_gates": [
    {
      "id": "D-CROSS",
      "status": "not_applicable_to_initial_milestone",
      "reason": "No physical-board support claim is authorized; a newly claimed target requires actual recorded target/hardware acceptance."
    }
  ],
  "changed_files": [],
  "failed_commands": [],
  "not_run_commands": [
    "G-BASE",
    "G-BUILD",
    "G-TEST",
    "G-EXAMPLES",
    "G-SDL",
    "G-SYS",
    "G-CODEGEN",
    "G-LVGL",
    "G-CORE",
    "G-NODEFAULT",
    "G-ALLOC",
    "G-FEATURES",
    "G-SIM",
    "G-PACKLIST",
    "G-DIFF",
    "D-CONFIG",
    "D-FEATURE",
    "D-SIZE",
    "D-PROVIDER",
    "D-PACKAGE",
    "D-LIFETIME",
    "D-GALLERY",
    "D-STATIC",
    "D-DOC"
  ],
  "blockers": [],
  "evidence": [],
  "stale_evidence": [],
  "authoring_validation": {
    "status": "passed",
    "counts_as_implementation_evidence": false,
    "files": [
      "docs/plans/README.md",
      "docs/plans/component-library-development-plan.md",
      "docs/plans/component-library-execution-ledger.md"
    ],
    "checks": [
      {
        "id": "documentation_structure_links_and_whitespace",
        "command": "python3 - <<'PY'\nfrom pathlib import Path\nlines = Path('docs/plans/component-library-development-plan.md').read_text().splitlines()\nstart = lines.index('<!-- documentation-checker:start -->')\nend = lines.index('<!-- documentation-checker:end -->')\ncode = '\\n'.join(lines[start + 2:end - 1])\nexec(compile(code, 'rfc-documentation-checker', 'exec'))\nPY",
        "cwd": "/Volumes/project_home/github/lv_binding_rust",
        "started_at": "2026-10-02T18:00:55Z",
        "finished_at": "2026-10-02T18:00:55Z",
        "exit_code": 0,
        "output": "PASS: 3 planning files, 10 contracts, 11 mapped requirements, 6 complete wave templates; links/fences/whitespace/placeholders checked."
      },
      {
        "id": "git_whitespace",
        "command": "git diff --check",
        "cwd": "/Volumes/project_home/github/lv_binding_rust",
        "started_at": "2026-10-02T18:00:55Z",
        "finished_at": "2026-10-02T18:00:55Z",
        "exit_code": 0,
        "output": "No whitespace errors reported."
      },
      {
        "id": "scope_ledger_and_checkout_fingerprint",
        "command": "python3 - <<'PY'\nfrom pathlib import Path\nimport hashlib, json, os, re, subprocess\nroot = Path.cwd()\nledger_path = 'docs/plans/component-library-execution-ledger.md'\nplan_path = 'docs/plans/component-library-development-plan.md'\nexpected_files = {ledger_path, plan_path, 'docs/plans/README.md'}\ndef git(*args):\n    return subprocess.check_output(['git', *args])\ndef sha(data):\n    return hashlib.sha256(data).hexdigest()\ndef canonical(value):\n    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()\ntext = Path(ledger_path).read_text()\nstate = json.loads(text.split('```json\\n', 1)[1].split('\\n```', 1)[0])\nplan = Path(plan_path).read_text()\nassert state['plan_status'] == 'draft' and state['implementation_authorized'] is False\nassert state['current_wave'] == 'W0' and state['current_work_package'] == 'W0.1'\nassert state['wave_state'] == 'not_started' and state['clean_acceptance_count'] == 0\nassert len(state['wave_states']) == 6 and set(state['wave_states'].values()) == {'not_started'}\nassert len(state['package_states']) == 22 and set(state['package_states'].values()) == {'not_started'}\nassert set(state['package_states']) == set(re.findall(r'^\\| (W\\d\\.\\d+) \\|', plan, re.M))\nassert len(state['required_gates']) == 24 and all(g['status'] == 'not_run' for g in state['required_gates'])\nassert set(state['not_run_commands']) == {g['id'] for g in state['required_gates']}\nassert set(re.findall(r'^\\| ([GD]-[A-Z]+) \\|', plan, re.M)) == {g['id'] for g in state['required_gates'] + state['conditional_gates']}\nassert state['last_completed_action'] is None\nassert all(state[key] == [] for key in ['changed_files', 'failed_commands', 'blockers', 'evidence', 'stale_evidence'])\nassert state['authoring_validation']['counts_as_implementation_evidence'] is False\nassert not git('diff', '--name-only', 'HEAD') and not git('diff', '--cached', '--name-only')\nuntracked_paths = [os.fsdecode(p) for p in git('ls-files', '--others', '--exclude-standard', '-z').split(b'\\0') if p]\nassert set(untracked_paths) == expected_files\nrecords = iter(git('status', '--porcelain=v1', '-z', '--untracked-files=all').split(b'\\0'))\nentries = []\nfor raw in records:\n    if not raw:\n        continue\n    entry = {'status': os.fsdecode(raw[:2]), 'path': os.fsdecode(raw[3:])}\n    if any(flag in entry['status'] for flag in 'RC'):\n        entry['source'] = os.fsdecode(next(records))\n    assert entry['status'] == '??' and entry['path'] in expected_files\n    if ledger_path not in (entry['path'], entry.get('source')):\n        entries.append(entry)\nentries.sort(key=lambda entry: (entry['path'], entry.get('source', ''), entry['status']))\nuntracked = []\nfor rel in sorted(untracked_paths):\n    if rel == ledger_path:\n        continue\n    path = root / rel\n    is_link = path.is_symlink()\n    data = os.fsencode(os.readlink(path)) if is_link else path.read_bytes()\n    untracked.append({'path': rel, 'type': 'symlink' if is_link else 'file', 'sha256': sha(data)})\nexclude = ':(exclude)' + ledger_path\nlock = root / 'Cargo.lock'\ncheckout = {\n    'revision': git('rev-parse', 'HEAD').decode().strip(),\n    'status_sha256': sha(canonical(entries)),\n    'relevant_diff_sha256': sha(git('diff', '--binary', 'HEAD', '--', '.', exclude) + b'\\0' + git('diff', '--cached', '--binary', '--', '.', exclude)),\n    'relevant_untracked_sha256': sha(canonical(untracked)) if untracked else 'none',\n    'lockfile_sha256': sha(lock.read_bytes()) if lock.exists() else 'absent',\n}\ncheckout['fingerprint'] = sha(canonical(checkout))\nassert checkout == state['checkout'], (checkout, state['checkout'])\nprint('PASS: exactly 3 new planning files; no tracked/staged changes; 6 waves and 22 packages not_started; 24 required gates not_run; no implementation evidence; checkout fingerprint matches ' + checkout['fingerprint'])\nPY",
        "cwd": "/Volumes/project_home/github/lv_binding_rust",
        "started_at": "2026-10-02T18:00:55Z",
        "finished_at": "2026-10-02T18:00:55Z",
        "exit_code": 0,
        "output": "PASS: exactly 3 new planning files; no tracked/staged changes; 6 waves and 22 packages not_started; 24 required gates not_run; no implementation evidence; checkout fingerprint matches 2c01b9c1dc86c07d192b84aa482891c7a9da6830a62ae3a8a748d030d993bd0d"
      }
    ],
    "completed_at": "2026-10-02T18:00:55Z",
    "limitations": [
      "Documentation authoring only; implementation remains unauthorized and not started.",
      "No builds, product tests, real UI, provider ABI, package-consumer builds, hardware or size measurements were run.",
      "Local Markdown destinations are checked; remote URLs and Markdown anchor resolution are not validated."
    ]
  }
}
```

## Evidence recording rules

Each future gate result records its exact command and cwd, UTC start/end time, exit status, expected/observed signal, checkout fingerprint, relevant toolchain/config/provider/lockfile identities, evidence file hashes and consecutive-pass count. Raw private configuration, complete environment dumps and secrets are not attached.

Mark affected records stale and reset their counters when relevant inputs change. Update this ledger before interruption, with one exact next action. Do not infer progress from prose summaries or from the planning-only checks.

## Pending decisions

- W0 / D-01: compatibility preset, release boundary, toolchain policy and minimal valid default-font/core closure.
- W1 / D-02: target metadata transport and receipt format, proven with host/target feature divergence.
- W2 / D-03: exact Linux ELF measurement target and commands.
- W3 / D-04: external-provider evidence and isolated package reconstruction commands.
- W4 / D-05: scoped handles, callback lifecycle, reentrancy and panic contract.
- D-06: fixture commands registered before the corresponding package exits.
- D-07: no real embedded target/board certification in the initial milestone; any later support claim adds its own required acceptance.
