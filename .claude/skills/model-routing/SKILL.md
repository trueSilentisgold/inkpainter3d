---
name: model-routing
description: Which model to give subagents and workflow agents in this project. Read before spawning any Agent or writing a Workflow script, so small tasks don't run on the most expensive model.
---

# Model routing for subagents

The owner pays for every token. The main conversation may run on the largest model; work
handed to subagents must not, unless it really needs it. **Always set `model` explicitly**
on every `Agent` call and every `agent()` in a workflow script (and on the phase entries in
`meta.phases`): an omitted model inherits the main model, which is the expensive one.

| Model | Use it for |
|---|---|
| `haiku` | Mechanical work with a clear recipe: renames, moving code, applying a pattern to many call sites, formatting, collecting file lists or grep results, running commands and reporting output, simple doc edits. |
| `sonnet` | The default for subagents: research (web, docs, crate sources), code audits, reviews and verification of findings, implementing a well-specified change in one area, drawing icons, driving the app under Xvfb and judging screenshots. |
| main model (omit `model` only here) | Nothing by default. Only when the owner asks for it, or for a design decision that sets architecture for later phases and that a `sonnet` attempt already got wrong. Say so in the reply when you do. |

The main conversation itself keeps coordination, decisions the owner should hear about,
and small edits it can do faster than briefing an agent.

## Also

- Prefer CI for mechanical checks (fmt, clippy, tests, gpu-smoke) over agents that run them.
- Fewer, well-briefed agents beat many: give each one the files, the theme/API it must use,
  and how to verify, so it doesn't spend tokens rediscovering the codebase.
- Reviews: one `sonnet` finder per area plus a `sonnet` verifier per finding is enough;
  don't run identical voters.
- When a workflow is interrupted (session limit), resume it with `resumeFromRunId` instead
  of starting over, and check `.claude/worktrees/` for uncommitted work first.
