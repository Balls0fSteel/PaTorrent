# Microsoft Store release (proposed D46)

Written 2026-11-01_070555_1793513155. A plan, not approved; no code before the owner answers the questions at the end. The Store facts were checked on 2026-11-01 against the sources listed at the bottom; confirm them in Partner Center before submitting, because Microsoft changes them.

## Facts that shape the plan

- **Account:** registration is free for individuals (since September 2025) and, per Microsoft's May 2026 post, for companies too. Individuals verify with an ID and a selfie.
- **Two ways in:**
  - **MSIX** (packaged): the Store signs the package for free, hosts it and updates it. The app must not update itself.
  - **EXE/MSI** (our NSIS setup as it is): we host the setup at a versioned HTTPS URL, it must install silently, and it must be Authenticode-signed with a certificate chaining to a trusted root (Azure Artifact/Trusted Signing, about USD 10 a month, or a CA certificate). The app updates itself, so D45 could stay.
- **Charging:** the Store's own payment handling (15% commission on apps) is documented for MSIX. For EXE/MSI, Partner Center lists Paid, but Microsoft answers and user reports say the Store does not take payment for them; we would need our own. Treat a paid EXE/MSI listing as unconfirmed.
- **Open source and price:** the 2022 rule against selling free/open-source software was withdrawn the same year; what remains targets copycats and irrational prices. The owner is the sole copyright holder (PolyForm Noncommercial binds others, not the owner; all dependencies are MIT), so a paid Store build is allowed. The listing should say it is the official build and that the same program is free on GitHub.

## Proposal

Ship an **MSIX** build for the Store, built by `release.yml` beside the zip and the setup, from the same `rookparcel.exe` with one build-time switch.

1. **A Store flavour (`ROOKPARCEL_STORE` at build time, like `ROOKPARCEL_NO_UPDATE_CHECK`):**
   - The D45 download-and-install path is compiled out; `updater::installed()` returns false.
   - The update check is off (the Store updates the app), and Help > Check for Updates opens the Store page (`ms-windows-store://pdp/?productid=<id>`) instead.
   - Portable mode (`rookparcel.ini` beside the exe) is ignored: the install folder is read-only.
   - About and the title show "(Microsoft Store)" so bug reports say where a copy came from.
2. **The package (`packaging/msix/`):**
   - `AppxManifest.xml`: `runFullTrust`, `internetClient`, `internetClientServer` and `privateNetworkClientServer` capabilities; a `windows.fileTypeAssociation` for `.torrent` and a `windows.protocol` for `magnet`; a `windows.startupTask` for "Start Rookparcel on system startup" (replacing the Run key in this flavour); the Verdigris logos at the Store's sizes (44, 150, 310x150, 310, 50 px plus scales).
   - Built with `makeappx` from the Windows SDK on the release runner; the upload is the unsigned `.msixupload`, which the Store signs. A locally signed test copy (a self-signed certificate) for hand checks.
3. **Behaviour to check inside the package:**
   - `%APPDATA%\Rookparcel`: MSIX redirects AppData writes to the package's private folder unless the manifest opts out. Decide whether the Store build shares settings with a setup-installed copy (opt out of virtualisation) or keeps its own.
   - Associations: Preferences > Windows integration writes the registry; in a package the manifest owns them, so that page shows the Windows default-apps settings instead.
   - The listening port: the Windows Firewall prompt still appears on first listen; check UPnP / NAT-PMP and uTP.
   - Moving completed data to other drives and NAS shares, `--remote`, the hosted Web UI, Auto Shutdown, the tray, `--minimized` at startup.
   - The nox Windows service is not offered (a packaged service needs admin and a different extension); nox stays in the zip.
4. **The listing:** reserve the name "Rookparcel" in Partner Center; a privacy policy page on the website (the update check and the Torznab, RSS and tracker traffic: nothing identifying is sent); the IARC age-rating questionnaire; screenshots from the demo (`--demo`), with no copyrighted titles; descriptions in the D24 languages, starting with English. Do not use µTorrent's name or logo in the title, icon or screenshots; mention it only for the importer.
5. **Release steps:** RELEASING.md gets a "Microsoft Store" step (upload the `.msixupload`, fill the submission's notes, wait for certification, usually 1-3 working days); `update.txt` is unaffected.

**Content policy:** torrent clients are listed in the Store. Rookparcel ships no search sources of its own (D41: the user's Prowlarr or Jackett only), which keeps it clear of "facilitates infringement". Certification may still ask; the submission notes should say so.

## Alternative: the existing setup as EXE

Less work in the app (D45 stays, no package behaviour to check), but it needs the yearly signing cost, and charging through the Store is unconfirmed. Signing would also fix the "Unknown publisher" UAC prompt and SmartScreen for the GitHub downloads, so it may be worth doing anyway (BACKLOG "Updates, installer and website").

## Open questions for the owner

**Answered 2026-11-04_224953_1793828993 (D46 approved):** Q1 free; Q2 MSIX; Q3 an individual account; Q4 the same `%APPDATA%\Rookparcel` as a setup-installed copy; Q5 all markets, the listing in all 34 languages; Q6 Verdigris is final (D57).

- **Q1.** Free or paid in the Store? If paid: price, and whether a free trial is offered (the Store supports time-limited trials for MSIX).
- **Q2.** MSIX (recommended: free signing, Store payments) or the signed EXE setup?
- **Q3.** Individual or company account (both free now; a company account shows a company name as the publisher, "DukeCorp1996")?
- **Q4.** Should the Store build share `%APPDATA%\Rookparcel` with a setup-installed copy, or keep its own settings?
- **Q5.** Which markets and languages at launch?
- **Q6.** Final brand: the provisional Verdigris icon becomes the Store icon, which is awkward to change once the listing has users.

## Sources (checked 2026-11-01)

- [Free developer registration for individual developers](https://blogs.windows.com/windowsdeveloper/2025/09/10/free-developer-registration-for-individual-developers-on-microsoft-store/)
- [Publish as a company with free registration (May 2026)](https://blogs.windows.com/windowsdeveloper/2026/05/07/publish-to-microsoft-store-as-a-company-now-with-free-registration-and-faster-onboarding/)
- [Create an MSI/EXE app submission](https://learn.microsoft.com/windows/apps/publish/publish-your-app/msi/create-app-submission)
- [Payments for an MSI or EXE Store app (Microsoft Q&A)](https://learn.microsoft.com/en-us/answers/questions/986498/how-to-recieve-payments-for-a-msi-or-exe-microsoft)
- [Microsoft reverts the open-source sales policy (TechCrunch, 2022)](https://techcrunch.com/2022/07/19/microsoft-u-turns-on-policy-that-wouldve-banned-commercial-open-source-apps/)
