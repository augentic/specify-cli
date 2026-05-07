---
id: propose
description: Turn discovery output into a sequenced milestone plan ready for /spec:define.
needs: [discovery]
generates: propose.md
---

# Propose brief

Take the discovery artifact and produce an ordered list of milestones with
owners, rough effort, and exit criteria. Downstream `/spec:define` runs one
slice per milestone.
