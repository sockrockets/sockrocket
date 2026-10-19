# README illustrations

These are documentation illustrations based on the project's Figma design direction and its companion vector layouts. They are not captures of a running proxy client and do not claim pixel parity with a release. Feature availability is described in the main README and the current application.

- `overview`: connection overview and traffic capture controls.
- `nodes`: node selection and subscription sources.
- `rules`: rule conditions and editing actions.
- `rules-bulk`: text entry, validation and confirmation.

Each desktop illustration PNG has an editable SVG source. All node names, subscription names, latency and traffic figures are fictional. Domains use the reserved example.com / example.org / example.net namespaces; the example CIDR is the documentation range 192.0.2.0/24. No production endpoints, personal paths, account details, subscription tokens, QR codes are included in the desktop illustrations. PNGs are rendered from these SVGs without retaining source metadata.

When updating these illustrations, use synthetic data from the outset. For actual interface captures, remove sensitive data before image export and verify the result; blur alone is insufficient.

## Merlin capture

`merlin.jpg` shows the actual Merlin management interface. The capture was rendered from a local sanitized copy: scripts, hidden configuration pages and credential inputs were removed; node identities, addresses, SSIDs and health-check details were replaced before image export. No unsanitized screenshot is retained. Runtime counters reflect capture time and are not synthetic or live telemetry. The redaction note is part of the image.
