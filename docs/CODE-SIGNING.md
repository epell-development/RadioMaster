# Code signing

Windows SmartScreen shows **Unknown publisher** and macOS Gatekeeper refuses to open RadioMaster the first
time because the downloads aren't signed with a publisher certificate. The publisher name in the exe and
installer (DovaFM Digital Entertainment Group) is already set, but Windows only trusts a name that comes
from a signature.

The release workflow (`.github/workflows/release.yml`) signs automatically once the settings below exist.
Without them it still builds everything, unsigned (Windows) or ad-hoc signed (macOS), as before.

## Windows: Azure Artifact Signing (formerly Trusted Signing)

The cheapest way to sign Windows programs today: about US$10 a month (Basic tier, 5,000 signatures). Microsoft
checks your identity once, then CI signs with a short-lived certificate in the name you verified. Check
Microsoft's current eligibility rules before paying: identity validation for individuals has been limited to
some countries (the US and Canada), and organisations need a few years of verifiable business history.

1. In the Azure portal create an **Artifact Signing account**, complete **identity validation**, and create a
   **Public Trust** certificate profile.
2. Create an app registration (Microsoft Entra ID → App registrations), add a client secret, and give it the
   **Artifact Signing Certificate Profile Signer** role on the signing account.
3. In GitHub → Settings → Secrets and variables → Actions add:
   - Secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`
   - Variables: `AZURE_SIGNING_ENDPOINT` (for example `https://eus.codesigning.azure.net/`, the region of
     your account), `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`

The next release then signs `RadioMaster.exe`, `RadioMaster-Server.exe` and `RadioMaster-Setup-<version>.exe`.

SmartScreen shows the publisher's name straight away, but a brand-new certificate can still get the blue
"Windows protected your PC" box for a while: SmartScreen builds reputation from downloads of signed files.
(EV certificates no longer skip that.)

Other options:
- **SignPath Foundation** signs open-source projects for free after an application; the publisher shown is
  "SignPath Foundation", not DovaFM. It needs its own GitHub integration (not wired up here).
- A classic **OV certificate** from a certificate authority costs roughly US$200–500 a year and since 2023
  must live on a hardware token or cloud HSM, which makes CI signing awkward.

## macOS: Apple Developer Program

US$99 a year (developer.apple.com/programs). An organisation account needs a D-U-N-S number; an individual
account signs in your own name.

1. Create a **Developer ID Application** certificate (Xcode → Settings → Accounts → Manage Certificates, or the
   developer website), then export it with its private key from Keychain Access as a `.p12` with a password.
2. Create an app-specific password at appleid.apple.com (for notarization).
3. Add GitHub secrets:
   - `APPLE_CERTIFICATE_P12`: the `.p12` as base64 (`base64 -i cert.p12 | pbcopy`)
   - `APPLE_CERTIFICATE_PASSWORD`: the `.p12` password
   - `APPLE_SIGNING_IDENTITY`: e.g. `Developer ID Application: DovaFM Digital Entertainment Group (TEAMID)`
   - `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: for notarization

`tools/package_macos.sh` then signs with the hardened runtime (`installer/macos/RadioMaster.entitlements`
allows the microphone), notarizes with Apple and staples the ticket, so the app opens with no warning.

## Without signing

The website and README tell people how to get past the warnings: on Windows **More info → Run anyway**; on a
Mac **System Settings → Privacy & Security → Open Anyway**. On Windows, updates installed from inside RadioMaster don't
show the warning.
