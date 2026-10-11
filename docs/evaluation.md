# Evaluating Lilith's accuracy

A test pack measures how well Lilith understands requests, so changes can be
judged by numbers instead of impressions.

## Run it

```powershell
cargo run --release -p lilith-brain --example eval -- --json out.json
```

Options:

- `--mode interpreter|agent`: `interpreter` (default) checks only the plan the
  interpreter produces and is fast (about 0.6 s per case). `agent` runs the
  whole loop against a recording dispatcher, also checks the parameters of the
  calls, and is about three times slower.
- `--cat NAME`: only one category (repeatable). `--limit N`: an evenly spaced
  sample. `--verbose`: also list the passing cases.
- Incremental by default: results are stored per case in
  `target/eval-state/`, and later runs execute only the cases that failed or
  are new/edited, reporting the whole pack with the stored results. Use
  `--all` for a full run (required for final numbers and before a PR, since
  regressions in previously passing cases are otherwise hidden).
- "core" metric: like "plan" but ignores preparatory steps that do not change
  the outcome (`window.list`, `window.focus`, `file.find`, `ui.find`).

The model is the one in `config.json` (or `LILITH_MODEL`).

## The pack

`crates/lilith-brain/eval/cases/*.json` holds specifications that the harness
expands: a phrase template with `{slots}` is multiplied by the slot values
(capped per template, deterministic). About 900 cases cover windows, audio,
apps, power, wallpaper, clipboard, files, web, browser (tabs, modes, search
kinds), chats (send, read, groups, several tasks), slang, memory, agents,
UI clicks, plain conversation and follow-ups, in Portuguese and English, with
typos. All names are placeholders ("user X", "group A").

A case lists the accepted plans (a plan is the ordered list of tools), and
optionally a `message` check (equals / contains / contains_any) for messages
she must send, expected `params` per tool (agent mode), and `context`
(previous requests, for follow-ups).

## Metrics

- **plan**: the plan equals one of the accepted plans, exactly.
- **core**: same, ignoring steps that only prepare the ground and don't change
  the outcome (`window.list`, `window.focus`, `file.find`, `ui.find`).
- **message**: the text she would send matches the check.
- **params**: the parameters of the expected call match.

Add a case whenever she gets something wrong, so it never regresses.

## What the interpreter learns

Three things are injected into the interpreter prompt per request, never all at once:

- **Plan examples**: up to three requests similar to the current one (word
  overlap), each with the tool plan that solved it. Built-in seeds plus plans of
  requests that finished without errors (stored in the local database). The
  seeds are deliberately worded differently from the evaluation cases.
- **Personal glossary**: slang and nicknames, editable in Settings or taught in
  chat ("X means Y"). Only the words present in the request are injected.
- **Shortcuts**: short unambiguous commands (numeric volume, mute, minimize /
  maximize / restore, open an app) skip the model and go straight to the
  dispatcher, so they still pass through the guard.

`/wrong [note]` marks the last request as badly handled: it stops being used as
an example and is kept (with the note) as material for new evaluation cases.
