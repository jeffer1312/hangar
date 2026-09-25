# Executor — receiving a correction recipe

Read at step 7 of `executor.md`, when a REPROVA arrives.

1. Reproduce the cause: run the "Cause reproduced" steps and see the defect with your own eyes.
2. Sweep the siblings: `git grep` the symbol repo-wide; every caller with the same defect
   enters this correction, in one pass. A list from the recipe or kick-off is a starting point:
   run the discovering command yourself, check all that show up, report any divergence. A
   sibling left out on purpose is named in the report with the reason. Unit: recipe about a
   function → check the file; about a network module → check the route.
3. Apply the steps, run the proof, go back to step 4 of `executor.md`, freeze round R+1, send
   it to the reviewer. A proof REPROVA that changes no code → back to step 7 of `executor.md`
   with `Phase: prova` on the same stash, not to step 4.

Stop, report to the arbiter and wait instead, when:

- the recipe does not match the code (symbol missing, bug does not reproduce there, text
  arrived cut in half); the scope stays as it was, nothing is improvised;
- the recipe breaks something else — report with the evidence;
- you disagree with evidence — the arbiter decides; the reviewer is not debated.

## Report line

```
Cause reproduced: <what you observed running the recipe's steps, before the fix>
```
