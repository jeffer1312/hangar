# Consumption per role

Whoever opens a session measures its interval (the reviewer does it for the verifier; the
arbiter also measures his own period). Read at opening, closing and in the retrospective. No
periodic collection, price lookup or sweep of other people's conversations.

`orq` below = `~/.claude/skills/orquestrar/scripts/orq.py --dir <durable dir>`.

## Capture start and end

Use Hangar's collector. The plan records the Hangar checkout path. Create `<durable>/medicao/`
and fill every placeholder; the transcript is the session's confirmed absolute path, never a
tmux name or a glob.

```bash
uv run --directory <hangar>/backend --no-sync python -m app.orq_consumo snapshot \
  --provider <claude|codex|pi|omp|kimi> --transcript <absolute-path> \
  --role <role> --session <name> --model <observed-model> --effort <observed-effort> \
  > <durable>/medicao/<name>-inicio.json
```

- Capture before the first request; repeat at closing into `<name>-fim.json`. Check the exit
  code before recording the artifact.
- A session that already worked: the start marks only the observed period. Never invent a
  zeroed start.
- Session, role or model changed: close the current pair and open another with new file names.
  Same for delimiting Tasks or rounds: distinct intervals, never a total plus its parts.
- File not yet created: wait for its confirmed creation before the request. Lost session or
  unreachable source: record the gap; a missing end is never zero consumption.
- Model and effort are observed from the session, never deduced from the contract nor
  attributed per call.
- Invalid JSON, a reset counter or a half-written line block exact measurement; wait for the
  write to finish. A pair with no new usage is reported, not counted as proven consumption.

## Report

```bash
uv run --directory <hangar>/backend --no-sync python -m app.orq_consumo report \
  --pair <durable>/medicao/<session1>-inicio.json <durable>/medicao/<session1>-fim.json \
  --pair <durable>/medicao/<session2>-inicio.json <durable>/medicao/<session2>-fim.json \
  > <durable>/medicao/relatorio.json
```

- The report sums end minus start per role: uncached input, cache read, cache created, output,
  with sources, identity and intervals. Overlapping intervals of the same source, a swapped,
  truncated or rewritten file and incompatible counters fail explicitly.
- Preserve the transcripts; generate the report on the same machine, before moving or archiving
  the sources.
- The total covers the registered sources only. Subagents with their own transcript need their
  own pairs, linked to the role that opened them; without them, declare that coverage missing.
  Never look for children by directory or name coincidence.
- The events JSONL keeps its types; the report paths go to the journal through `orq log` and
  into the retrospective's request.
- The observed window includes waiting and measures no productivity. Tokens are not price nor
  subscription percentage. A quota variation belongs to the whole account; do not attribute it
  to this work without separate measurement.

## Comparing configurations

The retrospective uses this data in its waste and model-card sections. To test a model switch:
representative tasks, the same request, codebase and criteria in each authorized
configuration, two or three runs each; record versions, tokens split by cache and role,
rejections and verification result. Comparison runs spend quota and require authorization; the
retrospective never triggers them. One isolated run is an observation, not proof.
