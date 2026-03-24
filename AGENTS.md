# Coding Guidelines

`xmf` is a simple CLI app to track finance portfolios. We strive to keep the
code minimal and clean.

## Repo Structure

- Follow the conventional `rust` app structure in this repo.
- Additional app-specific notes:
  - `src/core` contains the essential business logic for this app.
  - `src/cli` has the UX and presentation logic for command line.
  - `src/providers` are the external service interactions.

## Rules

### Coding

- Always prefer idiomatic rust for your changes.
- Focus on the task at hand. Never mix feature change and refactoring together.
- Smaller commits are better.
- Respect the existing structure and convention in the repo.

### Design

- You must respect DRY, SOLID, and similar clean code practices.
- Do not introduce unnecessary dependencies.
- Code must be correct and performant.
- Remember you're pairing with another senior dev, ask when you're in doubt.
  Lead with a proposal.

### Testing

- Always add unit tests for a change.
