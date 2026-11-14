# Legacy crypto

Old network equipment often speaks only SSH algorithms that are no longer
considered safe. The **Legacy crypto** setting (`legacy_crypto: true` in a
host file or a folder's `_folder.yaml`, or "Legacy crypto" in the side
panel) lets a session reach such equipment. It is off by default and is
inherited like the other settings. A jump host uses its own setting, not
the setting of the host it leads to. Quick connections never use it.

## What the switch changes

With the switch **off**, a session offers only modern algorithms:

| Kind         | Offered, in order of preference                                                                                                                                                                                                                                                      |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Key exchange | `mlkem768x25519-sha256`, `curve25519-sha256`, `curve25519-sha256@libssh.org`, `ecdh-sha2-nistp256`, `ecdh-sha2-nistp384`, `ecdh-sha2-nistp521`, `diffie-hellman-group-exchange-sha256`, `diffie-hellman-group18-sha512` … `group15-sha512`, `diffie-hellman-group14-sha256` |
| Host key     | `ssh-ed25519`, `ecdsa-sha2-nistp256`/`384`/`521`, `rsa-sha2-512`, `rsa-sha2-256`                                                                                                                                                                                                     |
| Cipher       | `chacha20-poly1305@openssh.com`, `aes256-gcm@openssh.com`, `aes128-gcm@openssh.com`, `aes256-ctr`, `aes192-ctr`, `aes128-ctr`                                                                                                                                                        |
| MAC          | `hmac-sha2-512-etm@openssh.com`, `hmac-sha2-256-etm@openssh.com`, `hmac-sha2-512`, `hmac-sha2-256`                                                                                                                                                                                   |

With the switch **on**, a session also offers these, after the modern ones
(a device that speaks both still gets a modern algorithm):

| Kind         | Added                                                                                                 |
| ------------ | ----------------------------------------------------------------------------------------------------- |
| Key exchange | `diffie-hellman-group14-sha1`, `diffie-hellman-group-exchange-sha1`, `diffie-hellman-group1-sha1`     |
| Host key     | `ssh-rsa` (RSA with SHA-1), `ssh-dss` (DSA)                                                           |
| Cipher       | `aes256-cbc`, `aes192-cbc`, `aes128-cbc`, `3des-cbc`                                                  |
| MAC          | `hmac-sha1-etm@openssh.com`, `hmac-sha1`                                                              |

`ssh-rsa` host keys count as legacy, as in OpenSSH since 8.8: a device that
can only sign with SHA-1 needs the switch. RSA _login_ keys are not
affected: they sign with SHA-2 when the device supports it.

A session that uses any of these algorithms shows a **Legacy crypto**
marker in its header; hovering over it lists the algorithms, per jump host
and for the device.

## When negotiation fails

When a device has no algorithm in common with the session, the session
closes and shows what the device offered (key exchange, host key, cipher
and MAC). It says whether turning on Legacy crypto would help, or that it
would not, because the device offers only algorithms that are not
supported.

## Not supported

Support is best effort: it covers what the SSH library (`russh`) provides,
and the project does not fork or patch it. These outdated algorithms stay
unsupported even with the switch on, so a device that offers nothing else
of a kind cannot be reached:

- MACs: `hmac-md5`, `hmac-md5-96` and their `-etm` forms (every MD5-based
  MAC), `hmac-sha1-96` (truncated SHA-1), `hmac-ripemd160`, and UMAC
  (`umac-64`, `umac-128`).
- Ciphers: `arcfour`, `arcfour128`, `arcfour256` (RC4), `blowfish-cbc`,
  `cast128-cbc`, `rijndael-cbc@lysator.liu.se`.
- Key exchange: GSS-API (`gss-*`) and RSA key exchange (`rsa1024-sha1`).
- Host keys: X.509 certificates (`x509v3-sign-rsa`, `x509v3-sign-dss`).
- SSH protocol 1.

The core keeps the same list in `UNSUPPORTED_ALGORITHMS`
(`crates/core/src/session/algorithms.rs`).
