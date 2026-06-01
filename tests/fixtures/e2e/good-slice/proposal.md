# Proposal

## Why

Users need to authenticate with the service via a dedicated login crate so
that downstream crates can rely on a consistent session identity.

## Source

Manual.

## What Changes

- Add a `login` crate that accepts a username and password.

## Units

### New Units

- `login`

## Impact

Bootstraps authentication for the application.
