# Design-refs baseline

### Requirement: User can log in

ID: REQ-001

Authentication.

#### Scenario: Valid credentials

- GIVEN a registered user
- WHEN they submit correct credentials
- THEN they receive a session token

### Requirement: User can log out

ID: REQ-002

Logout.

#### Scenario: Active session

- GIVEN an authenticated user
- WHEN they log out
- THEN the session is invalidated
