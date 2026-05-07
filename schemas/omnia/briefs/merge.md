---
id: merge
description: Merge the slice into the repository
needs: [build]
---

Before merging, confirm all task checkboxes in `tasks.md` are complete and the
slice status is `complete`. The merge skill handles delta-spec merging via
`merge-specs.py` and runs a baseline coherence check afterward.
