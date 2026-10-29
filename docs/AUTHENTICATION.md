# Authentication

RustCraft signs in with a Microsoft account. It uses the same chain of
services as the official launcher, and it checks that the account owns
Minecraft: Java Edition before it launches the official game. It never sees
or stores your password: you type that into Microsoft's own page in your web
browser.

The code is in `crates/rustcraft-auth`.

## The sign-in flow

1. **Browser sign-in (OAuth 2.0 authorization code with PKCE).** RustCraft
   starts a one-use web server on `127.0.0.1` with a random port. It then
   opens your default browser at
   `https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize`. The
   request includes:
   - the scope `XboxLive.signin offline_access`,
   - a random `state`,
   - an S256 PKCE challenge,
   - the redirect address `http://localhost:<port>`.

   Microsoft sends the browser back to that address with a one-time code.
   The local server ignores any request whose `state` does not match, and it
   stops after 5 minutes or when you press Cancel. If the browser does not
   open, the launcher shows the link to copy.
2. **Token exchange.** The code and the PKCE verifier are exchanged for a
   Microsoft access token and a refresh token.
3. **Xbox Live user token** from `user.auth.xboxlive.com`.
4. **XSTS token** from `xsts.auth.xboxlive.com`, for the relying party
   `rp://api.minecraftservices.com/`. Account problems come back here as
   `XErr` codes, and RustCraft turns them into readable messages (see below).
5. **Minecraft access token** from
   `api.minecraftservices.com/authentication/login_with_xbox`.
6. **Ownership check** at `/entitlements/license`. If the account does not
   own Java Edition, sign-in stops with "this Microsoft account does not own
   Minecraft: Java Edition".
7. **Profile** at `/minecraft/profile`: player name, UUID and skin.
8. **Multiplayer privileges** at `/player/attributes`. If multiplayer is
   turned off for the account (for example by parental controls), the
   launcher refuses to start an instance that is set to join a server, and
   says why. Singleplayer is still allowed.

All requests use HTTPS only, with a 30-second timeout.

## What is stored, and where

| Item | Where | Notes |
|---|---|---|
| Microsoft refresh token | OS credential store: Windows Credential Manager, macOS Keychain, or the Secret Service on Linux (GNOME Keyring, KWallet) | Service `RustCraft`, entry `msa-refresh:<uuid>`. Microsoft replaces it on each renewal, and RustCraft saves the new one |
| Minecraft access token | Memory only | Valid for about 24 hours. RustCraft renews it when less than 5 minutes are left |
| Account list | `accounts.toml` in the data folder | Player UUID, name, skin URL and the time of the last online check. No secrets |

If no credential store is available, for example on a Linux system without a
Secret Service, RustCraft keeps the sign-in in memory for the current session
and shows a warning. It never falls back to writing tokens to a file.

Tokens are never written to logs. The launcher log shows the game command
line with the access token replaced by `<redacted>`.

## Accounts

- **Add account**: Accounts → "Add Microsoft account". You can add several
  accounts.
- **Switch**: select another account in the Accounts list. The next launch
  uses it.
- **Sign out**: removes the account from `accounts.toml` and deletes its
  refresh token from the credential store.
- **Expiry**: if the refresh token has expired or was revoked (for example
  after a password change), the launcher says "the saved sign-in has
  expired; please sign in again".

## Offline play

The launcher offers "Play offline (singleplayer only)" only when both of
these are true:

- the online check failed because of a network problem, and
- the account passed a full online check in the last 30 days.

An offline launch starts the already-installed game with the account's name
and UUID and a placeholder token. Servers and Realms do not work in that
mode, because the game is not authenticated. RustCraft never creates offline
accounts, and it never shows an offline session as signed in.

The native RustCraft engine plays local worlds without any account, because
it uses no Mojang code or assets. If you are signed in, it uses your player
name.

## Configuring a build: the Azure application

Every Microsoft sign-in needs an application ID (a "client ID") registered
in Microsoft Entra ID (Azure). The ID is public, not a secret, and RustCraft
uses no client secret. Mojang must also approve the ID before the Minecraft
services accept it.

RustCraft does not include an ID. Set `RUSTCRAFT_MSA_CLIENT_ID` to choose one:

- **At run time:** `RUSTCRAFT_MSA_CLIENT_ID=<id> rustcraft`
- **At build time:** `RUSTCRAFT_MSA_CLIENT_ID=<id> cargo build --release`.
  The release workflow reads it from the repository variable
  `RUSTCRAFT_MSA_CLIENT_ID`.

A value set at run time overrides the built-in value. Without either, the
Accounts screen says "Not configured in this build", and launches of the
official game stop with a message that points here.

### Register the application

1. In the Azure portal, open **Microsoft Entra ID → App registrations → New
   registration**.
2. For **Supported account types**, select **Personal Microsoft accounts
   only**. Minecraft accounts are consumer accounts, and RustCraft signs in
   through the `consumers` endpoint.
3. Under **Redirect URI**, select the platform **Public client/native
   (mobile & desktop)** and enter `http://localhost`. Microsoft accepts any
   port for a `localhost` redirect address, which RustCraft needs because it
   picks a free port each time.
4. Do not create a client secret or certificate. A desktop application
   cannot keep a secret hidden.
5. Copy the **Application (client) ID**.

### Get Minecraft services approval

New application IDs are rejected by `login_with_xbox` until Mojang approves
them. Apply at <https://aka.ms/mce-reviewappid> with the ID from the previous
step. Until approval arrives, sign-in reaches step 5 and then stops with
"this build's Azure application is not approved for Minecraft sign-in".

## Error messages

| Message | Cause | What to do |
|---|---|---|
| Microsoft sign-in is not configured | No `RUSTCRAFT_MSA_CLIENT_ID` | See "Configuring a build" above |
| this build's Azure application is not approved | Mojang has not approved the client ID | Apply at <https://aka.ms/mce-reviewappid> |
| sign-in was declined in the browser | You cancelled or refused consent on Microsoft's page | Sign in again and accept |
| sign-in timed out waiting for the browser | No answer within 5 minutes | Try again; open the link shown in the launcher if no browser opened |
| This Microsoft account has no Xbox profile | XErr 2148916233 | Sign in once at xbox.com to create a profile |
| Xbox Live is not available in this account's country or region | XErr 2148916235 | The account's region cannot use Xbox Live |
| This is a child account… | XErr 2148916238 | An adult must add the account to a Microsoft family group |
| a parent or guardian must allow online play | XErr 2148916229 | Change the setting at account.xbox.com |
| Accept the Xbox terms of service | XErr 2148916234 | Accept them at xbox.com |
| Xbox requires additional verification | XErr 2148916236 or 2148916237 | Complete the verification at xbox.com |
| banned from Xbox services | XErr 2148916227 | Contact Xbox support |
| does not own Minecraft: Java Edition | No entitlement found | Use the account that bought the game |
| owns Minecraft but has no profile yet | Profile endpoint returned 404 | Create a profile at minecraft.net |
| Minecraft services are rate limiting sign-ins | HTTP 429 | Wait a minute and try again |
| network error during … | No connection, DNS or TLS failure | Check the connection; offline play may be offered (see above) |

## Testing

`crates/rustcraft-auth/tests/flow.rs` runs the whole flow against mock
versions of every service, with a scripted "browser" that follows the
redirect. It also covers:

- a forged redirect with the wrong `state`,
- declined consent and cancelling,
- an Xbox account error (no Xbox profile),
- an application that Mojang has not approved,
- an account without ownership, and one without a profile,
- an expired refresh token,
- a multiplayer ban,
- token renewal.

Real credential store access is tested separately in CI against GNOME
Keyring (`RUSTCRAFT_TEST_KEYRING=1`).

The flow has not been run against the live Microsoft, Xbox and Minecraft
services yet. That needs an approved client ID and a test account.
