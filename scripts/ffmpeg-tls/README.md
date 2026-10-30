# Embedded Mozilla roots for the portable decoder

`prepare.py` prepares **FFmpeg 9.0.2** source for the static Mbed TLS build.
It never builds FFmpeg and never changes Mbed TLS cryptography.

```sh
python3 scripts/ffmpeg-tls/prepare.py \
    --source /path/to/ffmpeg-9.0.2 \
    --download-dir /path/to/downloads
```

Python 3 and the `patch` program are required. The source file must match the
pinned pristine hash or the exact patched hash; rerunning is safe. The helper
checks all input hashes, generates `libavformat/vk_fast_mozilla_ca.h` and applies
the small accompanying patch. The checked-in PEM permits offline preparation.
If it is absent, the helper uses the verified download cache or fetches the
immutable dated URL and verifies the pinned hash.

The generated header contains the complete original PEM bytes followed by a
single NUL, as required by `mbedtls_x509_crt_parse` for PEM input. UTF-8 names in
PEM comments use fixed-width octal C escapes. No certificate file or TLS DLL is
needed beside the resulting executable.

The FFmpeg patch loads these roots **only when `ca_file` is absent**. Explicit
CA files retain the upstream behavior. A bundle parse error aborts opening the
connection, including partial parse failures. The patch does not change
`tls_verify`, `verifyhost`, SNI, hostname verification, TLS negotiation or
post-handshake certificate checks. VK Fast enables verification for playback.
No verification bypass is introduced.

## Provenance and updates

The bundle is curl's conversion of Mozilla's CA store dated **2026-10-20**:
118 certificates, 200116 bytes. On 2026-10-30 both the current and dated
official PEM matched both official SHA256 listings:

```text
796f1389bfea13c0ed78f663b765baaff54a508e5483786bd6ece73c5e3e3cbe
```

- [curl's Mozilla CA extraction documentation](https://curl.se/docs/caextract.html)
- [Dated PEM](https://curl.se/ca/cacert-2026-10-20.pem)
- [Dated official SHA256](https://curl.se/ca/cacert-2026-10-20.pem.sha256)
- [Current PEM](https://curl.se/ca/cacert.pem)
- [Current official SHA256](https://curl.se/ca/cacert.pem.sha256)

`trust-source.json` records the immutable download, bundle hash, certificate
count and FFmpeg source/patch hashes. Updates require a reviewed new bundle,
manifest and decoder rebuild; the application does not update trust roots at
runtime. This static store does not import additional Windows enterprise roots
or the platform revocation policy. Mozilla constraints outside the converted
PEM are not represented; curl documents that limitation.

The original PEM and the generated CA-data header are licensed under MPL 2.0;
see `LICENSE-MPL-2.0.txt` and the license notice retained in the PEM. The FFmpeg
patch retains FFmpeg's existing LGPL-2.1-or-later terms. These files belong in
the source/license companion, while the runtime archive keeps only the player,
decoder and writable configuration/cache directories.

## Checks

```sh
python3 scripts/ffmpeg-tls/test_prepare.py --source /path/to/ffmpeg-9.0.2
```

These checks use an isolated source copy, verify idempotence and rejection of
tampered inputs, and compile a tiny C header test when `cc` is available. They
do not compile FFmpeg or make authentication requests.

The completed Windows decoder must separately pass playback over HTTPS using
the embedded roots, reject an untrusted self-signed server, accept that server
only with an explicit matching CA file, and reject a hostname mismatch even
with that CA file. The Windows build validation performs these network/TLS
checks on synthetic audio; source preparation alone cannot prove them.
