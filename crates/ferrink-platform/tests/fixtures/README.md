# Synthetic test fixtures

Every file in this directory is synthetic data for host-side parser and runtime
tests. It contains no device capture, endpoint, account data, document data,
or unique device identifier. The `reference-*` names are test labels only and
must not be used as a physical-device profile.

Synthetic probe reports still use Ferrink's production redaction policy name
and excluded-category vocabulary so they exercise the same strict validation
contract as reports collected on a device.
