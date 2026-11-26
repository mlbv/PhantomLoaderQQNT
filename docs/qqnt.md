<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# QQNT findings

The telemetry and security findings below were established on Windows x64 QQNT 9.9.33-52230, whose `resources/app/wrapper.node` has SHA-256 `63112ab9161e127f5f7e17998a7196e143808923fb54cbbf7b4e21426187a5f0`. That build contains 1,005 registered serialized V8 caches in `major.node` and uses V8 14.4.258.16.

The current [compatibility profile](../distribution/profile.json) targets QQNT 9.9.38-54985. The bounded incremental review described below was performed on 9.9.37-54395, and the telemetry routes and earlier security findings retain their original build scope unless that review explicitly confirms the corresponding newer path.

PhantomLoaderQQNT 1.2.0 provides LiteLoader compatibility version 5.2.1, and each plugin selects its facade in the manifest before receiving it through `onLoad(api)`. Both facades share plugin state and storage; the native binding is `phantomloader_native`, with `liteloader_native` retained as an alias. The compatibility profile identifies the verifier and loader-hook targets for this module set, while core configuration remains in `data/LiteLoader/config.json`; the `local://` resource scheme and synchronous preload transport remain available to both API facades.

## QQNT 9.9.37-54395 update review

The newer wrapper has SHA-256 `fed532ad58d2404dcedf2858554e8fa52a5f4e65b271c008d2faca4011d7d4a2`. Its security configuration now accepts two additional keys, `DCAttest` and `DCDeviceToken`, whose defaults are enabled in the constructor; those defaults alone do not establish that either operation runs under the effective client configuration. The inherited `PCCodeSumDetect` route compares selected blocks of the loaded wrapper and QQNT engine against their files on disk and reports differing byte samples. Its scan has exclusions and incomplete block coverage; effective runtime configuration and all indirect activation paths remain unresolved.

Inspection extends beyond ordinary module enumeration: a wrapper security routine walks native stacks using Windows unwind APIs, associates instruction addresses with modules, and can set a security flag through the signing path. Another security collector queries process handles and named objects, while the updated AMS SDK exposes a `GetReportAntiCheatInfo` interface whose inspected inputs include executable signing information, version resources and application identity. The recovered AMS interface builds report tokens; its name does not establish a scan of private executable mappings.

The handle collector also adds Thread objects and replaces inline name checks with a larger table, but that table stores narrow bytes which its matcher interprets as UTF-16. Consequently, the added marker names do not establish effective matching of their ordinary ASCII spellings, so the apparent expansion cannot be described as stronger detection.

Selected inspection-API callers in `QQNT.dll` instead perform Node diagnostic reporting, Chromium resident-memory accounting, profiler stack-range setup and V8/Crashpad exception handling. The embedded cache inventory also grows from 1,005 to 1,057 entries, although extensive numeric-key renumbering prevents interpreting that difference as 52 identified new logical modules. Three selected newer caches were deserialized for bytecode inspection, recovering local-gateway secret persistence and game-center webview authentication rather than a loader-detection predicate in those functions.

The current runtime applies exact-build patches to the signing-stack, code-integrity, handle-inspection, environment-collection, self-region-collection and parent-identity routes before normal wrapper initialization. These preserve the original signing continuation and the checksum collector's existing disabled result, return a valid empty handle result, force the environment collector's empty branch so the environment record is not constructed, report an empty private-executable-region list, and take the parent probe's no-parent path; the patches cover only the selected paths.

This review establishes specific mechanisms and changes within the inspected code, while unexamined native and cached functions, indirect activation and server-supplied programs remain coverage gaps. Functional compatibility and successful launches therefore remain separate from resistance to the [inspection methods observable to QQNT](../native/README.md#inspection-limits).

## Telemetry

The build contains native OTLP-style metrics, traces and logs. Metrics and traces use `tpstelemetry.tencent.com`, with `/v1/metrics` and `/v1/trace` routes, while logs use `galileotelemetry.tencent.com/v1/logs`. Native metrics and logs use protobuf with Content-Type `application/x-protobuf`; the metrics controller collects at ten-second intervals, and the trace startup path enables export eligibility.

The embedded JavaScript adds Aegis collection for Galileo telemetry, Oceanus metrics for Tencent's telemetry service, Beacon event reporting, diagnostic key/value requests, game-center activity reports and performance events. Beacon uploads use `otheve.beacon.qq.com` domestically and `svibeacon.onezapp.com` overseas, while `oth.str.beacon.qq.com` supplies strategy and configuration. The JavaScript Aegis path carries the authenticated account identifier through its renderer flow, whereas the main-process envelope identifies itself as `QQMain`.

Native media and SDK components add reporting routes for AVSDK, Shiply, AMS and Qimei, including `oth.eve.mdt.qq.com`, `trace.inlong.qq.com` and `sdkreport.e.qq.com`. Game-center activity uses `report.gamecenter.qq.com`; crash reporting includes a legacy WinINet path at `bugreportv2.qq.com` and a modern crash-handler path rooted at `rmonitor.qq.com`, where the modern path negotiates an upload URL. The build also contains initialized Bugly service roots, replay-service origin entries and XNN report-server configuration at `r.avlab.qq.com`.

## Security reporting and DLVM

The qsec/O3 subsystem provides cached-report and task-response submission paths through the MSF/SSO transport. Task responses carry module information, environment records and server-selected results, while the module-reporting task handles heartbeat work and collects module information. The fixed environment provider reports virtualization and host-environment markers, including an exact lookup for `frida-agent.dll`.

The same subsystem contains a server-selected DLVM task engine that can read and write native memory, allocate memory, operate on full-width integer values and perform indirect native calls. It accepts binary programs and reports processing stages through the qsec reporting route.

## Native capture

Core startup arms DLVM capture before third-party plugins load, so a matching downloaded program is saved under the profile capture directory before QQNT exits. The local report window then presents the capture result, opens a prefilled GitHub issue and leaves the user to attach the offered ZIP manually; original captures remain under `captures`, while report ZIPs are stored under `captures/reports`.

The native API exposes native address lookup, integer-ABI calls, inline hooks, synchronous capture control and capture status. NeoHook with iced-x86 instruction relocation supplies the hook backend, while attachments belong to their creating Node environment and environment cleanup detaches its remaining hooks. The compatibility profile defines the supported verifier, hook, task-layout and capture sites.

## Interception

`PhantomLoader.api.intercept` registers main-process transformations for incoming QQ calls, outgoing QQ command dispatch and external URLs. Handlers receive the original arguments together with channel and window context, associated web contents and normalized QQ command views, so they can continue with the current arguments, replace the complete argument list or cancel the operation. The registry and transport adapter preserve priority order, asynchronous stream ordering, owner disposal and web-contents cleanup.

The loader's public APIs define compatibility, native hooks, capture control and message interception; the telemetry and security mechanisms described here remain QQNT implementation findings with the build scope stated above.
