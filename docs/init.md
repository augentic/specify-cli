# `specify init`

`specify init` scaffolds the per-project `.specify/` tree plus
`project.yaml`. It has two mutually exclusive shapes; missing both
surfaces as `init-requires-capability-or-hub`.

## Regular project — `specify init <capability>`

Pass a capability identifier or a directory/URL that resolves to one:

```bash
specify init omnia
specify init https://github.com/augentic/omnia.git
specify init ./path/to/capability
```

The capability supplies the schemas, plan template, and registry hooks
the project will use. The CLI writes:

- `project.yaml` (capability identifier, `specify_version` floor).
- `.specify/` (slices, archive, plans, cache, workspace, plan.lock).

## Platform hub — `specify init --hub`

```bash
specify init --hub --name <hub-name>
```

A hub is a registry-only project: it owns `registry.yaml` and the
cross-repo workspace, but does not itself host capability artifacts.
Use this for the platform repo that orchestrates a fleet of capability
projects.

## Why the two shapes are exclusive

A capability project pins one capability identifier; a hub pins none
(it owns the registry of many). Mixing the two would produce a
`project.yaml` whose semantics depend on whether downstream verbs
treat the project as a capability source or as a registry root, and
different verbs would disagree. The CLI refuses the ambiguous shape
at the entry point with the `init-requires-capability-or-hub`
discriminant.
