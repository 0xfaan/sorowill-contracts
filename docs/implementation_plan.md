# Implementation Plan - Add SECURITY.md Policy (Issue #99)

## Goal Description
The repository `SoroWill/sorowill-contracts` manages smart contracts that custody user funds on Stellar Soroban. To ensure security researchers have a documented, confidential method to report security vulnerabilities, this plan outlines adding a `SECURITY.md` file with a clear responsible-disclosure policy and linking it from `README.md`.

## Proposed Changes

### Documentation

#### [NEW] [SECURITY.md](file:///C:/Users/%D0%9C%D0%B0%D0%BA%D1%81%D0%B8%D0%BC/Documents/antigravity/valiant-hopper/sorowill-contracts/SECURITY.md)
Create `SECURITY.md` containing:
- **Supported Versions**: Clearly state supported branches/deployments (e.g. latest `main` branch and active Testnet contract deployments).
- **Reporting a Vulnerability**: Document private vulnerability reporting via GitHub Private Vulnerability Reporting or security email (`security@sorowill.org`).
- **Response SLAs**: Initial acknowledgement within 48 hours, status updates every 5 business days, and resolution targets based on severity.
- **In-Scope & Out-of-Scope**: Explicit list of smart contracts (`contracts/will`), deployed WASM artifacts, and out-of-scope third-party infrastructure.
- **Responsible Disclosure & Rewards**: Guidelines for safe disclosure without disrupting user assets, and inclusion in the Stellar Wave / Drips security reward pool.

#### [MODIFY] [README.md](file:///C:/Users/%D0%9C%D0%B0%D0%BA%D1%81%D0%B8%D0%BC/Documents/antigravity/valiant-hopper/sorowill-contracts/README.md)
- Add a Security badge in the top badges section pointing to `./SECURITY.md`.
- Add a `## Security Policy` section referencing `SECURITY.md` for private disclosures.

## Verification Plan
1. Validate Markdown rendering and check all links (`./SECURITY.md`).
2. Verify git status and ensure branch adheres to repository guidelines.
