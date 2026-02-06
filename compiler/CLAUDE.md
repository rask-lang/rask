
## Error messages

- Simple errors are kept brief
- Whereas complex errors come with explanations and examples
- And, when possible, with suggestions for a fix
- For very complex errors, we give an in-depth technical explanation

## Axioms for Human-Centric Errors

- Mirroring: Display raw source code; never force the user to mentally de-map "pretty-printed" output.

- Visual Locality: Prioritize inline visual pointers over abstract x,y coordinates.

- Symptom-to-Intent: Translate "compiler state" (what happened) into "user intent" (what was desired).

- Mandatory Guidance: Every error must include a specific, actionable hint.

- Information Hierarchy: Use layout/whitespace to facilitate scanning; reveal complexity only as needed.

- Functional Chromatics: Apply color strictly to categorize data (e.g., Red = Error, Blue = Separator).

- Structural Portability: Errors must be machine-readable (JSON) to enable IDE-native feedback.

- Zero-Cost UX: High-quality feedback should leverage existing metadata without degrading algorithm performance.

- Feedback Loops: Error quality is dynamic; maintain a "catalog" of failures to iterate on clarity.