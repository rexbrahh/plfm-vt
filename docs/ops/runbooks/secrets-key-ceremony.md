# Secrets Master Key Ceremony Runbook

Last updated: 2025-12-17

## Purpose

This runbook defines the procedures for:
- Initial master key generation
- Master key storage and access
- Key rotation
- Key recovery
- Audit and compliance

The secrets master key protects all customer secret bundles at rest.

## Prerequisites

- At least 2 authorized operators with MFA-enabled accounts
- Access to the key management infrastructure
- Secure, air-gapped workstation for key generation
- Hardware security module (HSM) or approved key storage

## Key Architecture

### Key Hierarchy

```
Master Key (root)
    └── Data Encryption Keys (DEKs)
            └── Secret Bundle Ciphertext
```

- **Master Key**: Long-lived key that encrypts DEKs. Stored in HSM or SOPS.
- **DEKs**: Per-secret-bundle keys. Encrypted by master key. Stored alongside ciphertext.
- **Secret Bundles**: Customer secrets encrypted by DEK.

### Key Material

| Key Type | Algorithm | Size | Rotation Period |
|----------|-----------|------|-----------------|
| Master Key | AES-256-GCM | 256 bits | Annual or on compromise |
| DEK | AES-256-GCM | 256 bits | Per secret version |

## Initial Key Generation Ceremony

### Participants

- **Key Custodian 1**: Generates key material
- **Key Custodian 2**: Witnesses and verifies
- **Security Officer**: Approves and audits

### Environment Preparation

1. Use an air-gapped workstation with:
   - No network connectivity
   - Fresh OS installation
   - Verified checksums for all software

2. Required software:
   - `age` (encryption tool)
   - `sops` (secrets operations)
   - `openssl` (verification)

3. Verify software integrity:
   ```bash
   sha256sum $(which age) $(which sops) $(which openssl)
   # Compare against published checksums
   ```

### Key Generation Steps

1. **Generate master key** (Key Custodian 1):
   ```bash
   # Generate age keypair
   age-keygen -o master-key.txt
   
   # Extract public key
   grep "public key:" master-key.txt
   
   # Generate key ID (first 8 chars of public key hash)
   cat master-key.txt | grep "public key:" | awk '{print $3}' | sha256sum | cut -c1-8
   ```

2. **Verify generation** (Key Custodian 2):
   ```bash
   # Verify key file format
   head -1 master-key.txt  # Should show "# created:"
   
   # Test encryption/decryption
   echo "test" | age -r $(grep "public key:" master-key.txt | awk '{print $3}') | age -d -i master-key.txt
   ```

3. **Create key backup shares** (both custodians):
   ```bash
   # Split key into 3 shares, requiring 2 to reconstruct
   # Using Shamir's Secret Sharing
   ssss-split -t 2 -n 3 < master-key.txt
   ```

4. **Secure storage**:
   - Share 1: HSM slot A (Key Custodian 1)
   - Share 2: HSM slot B (Key Custodian 2)
   - Share 3: Secure offline storage (Security Officer)

5. **Destroy working copies**:
   ```bash
   shred -u master-key.txt
   ```

### Verification

1. **Test key reconstruction** (both custodians, separate session):
   ```bash
   ssss-combine -t 2 > reconstructed-key.txt
   # Enter 2 shares
   
   # Verify by decrypting test ciphertext
   age -d -i reconstructed-key.txt test-ciphertext.age
   
   # Destroy reconstructed key
   shred -u reconstructed-key.txt
   ```

2. **Document ceremony**:
   - Record date, time, location
   - Record participant identities (verified via ID)
   - Record key ID (public key hash prefix)
   - Sign attestation document

## SOPS Configuration

### Initial Setup

1. Create `.sops.yaml` in repository:
   ```yaml
   creation_rules:
     - path_regex: secrets/.*\.yaml$
       age: >-
         age1abc123...  # Master key public key
   ```

2. Test encryption:
   ```bash
   sops --encrypt --age age1abc123... secrets/test.yaml > secrets/test.enc.yaml
   sops --decrypt secrets/test.enc.yaml
   ```

### Production Configuration

Control plane reads master key from:
- Environment variable: `PLFM_SECRETS_MASTER_KEY` (for HSM-backed deployments)
- SOPS keyservice connection (preferred)

## Key Rotation Procedure

### Planned Rotation (Annual)

1. **Schedule rotation window** (2 weeks notice):
   - Notify all operators
   - Schedule ceremony participants
   - Prepare new HSM slots

2. **Generate new master key** (follow ceremony above)

3. **Re-encrypt all DEKs**:
   ```bash
   plfm admin secrets rotate-master-key \
     --old-key-id <old-id> \
     --new-key-id <new-id>
   ```

4. **Verify re-encryption**:
   ```bash
   plfm admin secrets verify-integrity
   ```

5. **Update SOPS configuration**

6. **Retire old key after grace period** (30 days)

### Emergency Rotation (Key Compromise)

1. **Declare incident** - Page security team

2. **Generate new key immediately** (single custodian acceptable in emergency)

3. **Re-encrypt all secrets**:
   ```bash
   plfm admin secrets emergency-rotate --force
   ```

4. **Revoke old key access**:
   - Remove from HSM
   - Update access policies
   - Rotate any derived credentials

5. **Notify affected customers** (per incident policy)

6. **Post-incident**: Follow standard ceremony to establish proper key shares

## Key Recovery Procedure

### When to Use

- HSM failure
- Lost key share
- Disaster recovery

### Recovery Steps

1. **Gather custodians** (minimum 2 share holders)

2. **Verify identity** (MFA + ID check)

3. **Reconstruct key**:
   ```bash
   ssss-combine -t 2 > recovered-key.txt
   ```

4. **Verify key** by decrypting known test ciphertext

5. **Re-establish shares** if any were lost

6. **Secure and destroy working copy**

### Recovery Time Objective (RTO)

- Target: 4 hours
- Maximum: 24 hours

## Audit Requirements

### Ceremony Records

Maintain signed records of:
- All key generation ceremonies
- All rotation events
- All recovery events
- Participant identities

Retention: 7 years minimum

### Access Logs

Log all:
- Master key access attempts
- DEK operations
- Decryption operations

### Compliance Checks

Quarterly:
- Verify key share integrity (sealed envelope inspection)
- Review access logs
- Test recovery procedure (tabletop)

Annually:
- Full recovery drill
- Key rotation (even if not required)
- External audit

## Incident Response

### Suspected Key Compromise

1. Page security team immediately
2. Do NOT rotate key until forensics approves
3. Preserve all logs and evidence
4. Follow emergency rotation once approved

### Lost Key Share

1. Notify security team
2. Schedule key rotation within 7 days
3. Investigate loss cause
4. Update procedures as needed

## CLI Commands

```bash
# Check key status
plfm admin secrets key-status

# List key versions
plfm admin secrets list-keys

# Verify all secrets integrity
plfm admin secrets verify-integrity

# Rotate master key (planned)
plfm admin secrets rotate-master-key --new-key-id <id>

# Emergency rotation
plfm admin secrets emergency-rotate --force

# Export key metadata (not key material)
plfm admin secrets export-metadata > key-metadata.json
```

## Contacts

- Primary Key Custodian: [Name, contact]
- Secondary Key Custodian: [Name, contact]
- Security Officer: [Name, contact]
- On-call escalation: [PagerDuty service]

## References

- `docs/specs/secrets/encryption-at-rest.md`
- `docs/security/03-secret-handling.md`
- [age encryption tool](https://github.com/FiloSottile/age)
- [SOPS documentation](https://github.com/mozilla/sops)
