# Blocking API benchmarks

## 1.4.0 async and blocking comparison

**Measured on October 12, 2026:** Intel Core i7-9700F, Linux x86_64,
Rust 1.98.1 and Tokio 1.53.1; release builds in a private network namespace.
The same async echo server serves both API clients. Server CPUs 0–3 and
client CPUs 4–7 each run four Tokio workers. The blocking client shares one
runtime and adds one application thread per connection; the async client uses
Tokio tasks. Blocking is measured at 1 and 64 connections; larger counts here
use the async API.

Both drivers are built with `blocking`; send/recovery policies, receive batching
and idle maintenance are disabled. They use protocol version 11, the default
1,400 B nominal MTU, `ReliableOrdered`, 20 warmup echoes per connection and a
16-message sliding application window. Batch calls submit only ready messages.
Every measured message has a timestamp and verified connection ID, message ID
and full payload. The timer excludes setup/warmup; RSS includes the entire
process lifetime and RTT sample storage. All **120 measurements** passed echo
validation, separately from eight smoke measurements.

Each cell lists **throughput / 99% RTT / peak RSS (C: client, S: server)**.
Values are three-run medians; parentheses are min–max ranges, including units.
Injected loss affects both directions and includes control packets. Added RTT
is twice the per-direction delay; it excludes processing time.

| Connections / payload / loss / injected RTT | Async send | Async batch | Blocking send | Blocking batch |
|---|---|---|---|---|
| 1 / 64 B / 0% / 0 ms | 14.16 MiB/s (14.10–14.49 MiB/s)<br>99% RTT: 0.14 ms (0.12–0.15 ms)<br>C: 12.48 MiB (12.26–12.54 MiB)<br>S: 4.09 MiB (3.91–4.10 MiB) | 34.12 MiB/s (33.10–34.60 MiB/s)<br>99% RTT: 0.12 ms (0.07–0.13 ms)<br>C: 12.45 MiB (12.44–12.57 MiB)<br>S: 4.07 MiB (3.91–4.13 MiB) | 14.33 MiB/s (14.32–15.01 MiB/s)<br>99% RTT: 0.19 ms (0.14–0.19 ms)<br>C: 12.70 MiB (12.61–12.82 MiB)<br>S: 4.05 MiB (3.95–4.23 MiB) | 24.94 MiB/s (13.93–28.59 MiB/s)<br>99% RTT: 0.21 ms (0.11–0.36 ms)<br>C: 12.55 MiB (12.50–12.69 MiB)<br>S: 4.21 MiB (4.06–4.31 MiB) |
| 1 / 800 B / 0% / 0 ms | 163.70 MiB/s (162.01–164.56 MiB/s)<br>99% RTT: 0.18 ms (0.16–0.18 ms)<br>C: 9.47 MiB (9.39–9.60 MiB)<br>S: 4.18 MiB (3.97–4.21 MiB) | 167.31 MiB/s (163.20–167.68 MiB/s)<br>99% RTT: 0.14 ms (0.14–0.17 ms)<br>C: 9.64 MiB (9.60–9.72 MiB)<br>S: 4.20 MiB (4.10–4.21 MiB) | 159.25 MiB/s (158.92–160.04 MiB/s)<br>99% RTT: 0.23 ms (0.22–0.26 ms)<br>C: 9.62 MiB (9.59–9.64 MiB)<br>S: 4.15 MiB (4.14–4.21 MiB) | 161.11 MiB/s (159.53–166.21 MiB/s)<br>99% RTT: 0.21 ms (0.16–0.23 ms)<br>C: 9.66 MiB (9.66–9.67 MiB)<br>S: 4.33 MiB (4.13–4.38 MiB) |
| 1 / 4096 B / 0% / 0 ms | 279.75 MiB/s (269.33–282.15 MiB/s)<br>99% RTT: 0.81 ms (0.58–0.93 ms)<br>C: 5.42 MiB (5.42–5.57 MiB)<br>S: 4.67 MiB (4.67–4.70 MiB) | 281.25 MiB/s (134.40–290.35 MiB/s)<br>99% RTT: 0.83 ms (0.55–0.89 ms)<br>C: 5.50 MiB (5.47–5.61 MiB)<br>S: 4.60 MiB (4.55–4.62 MiB) | 306.42 MiB/s (300.28–308.43 MiB/s)<br>99% RTT: 0.68 ms (0.42–0.80 ms)<br>C: 5.38 MiB (5.22–5.68 MiB)<br>S: 4.45 MiB (4.32–4.71 MiB) | 321.48 MiB/s (318.85–324.94 MiB/s)<br>99% RTT: 0.61 ms (0.60–0.75 ms)<br>C: 5.59 MiB (5.49–5.75 MiB)<br>S: 4.52 MiB (4.46–4.55 MiB) |
| 1 / 800 B / 1% / 0 ms | 88.94 MiB/s (69.95–90.01 MiB/s)<br>99% RTT: 0.46 ms (0.41–0.62 ms)<br>C: 5.11 MiB (5.08–5.28 MiB)<br>S: 4.27 MiB (4.27–4.31 MiB) | 58.47 MiB/s (51.93–62.68 MiB/s)<br>99% RTT: 0.63 ms (0.54–0.64 ms)<br>C: 5.12 MiB (4.99–5.16 MiB)<br>S: 4.30 MiB (4.23–4.39 MiB) | 130.09 MiB/s (127.19–131.23 MiB/s)<br>99% RTT: 0.44 ms (0.43–0.48 ms)<br>C: 5.28 MiB (5.20–5.28 MiB)<br>S: 4.35 MiB (4.34–4.49 MiB) | 123.31 MiB/s (119.37–127.34 MiB/s)<br>99% RTT: 0.45 ms (0.44–0.51 ms)<br>C: 5.34 MiB (5.02–5.43 MiB)<br>S: 4.25 MiB (4.14–4.32 MiB) |
| 1 / 800 B / 5% / 0 ms | 11.23 MiB/s (10.81–24.34 MiB/s)<br>99% RTT: 1.39 ms (0.96–1.41 ms)<br>C: 4.27 MiB (4.16–4.39 MiB)<br>S: 4.20 MiB (4.16–4.35 MiB) | 11.86 MiB/s (11.46–14.42 MiB/s)<br>99% RTT: 1.39 ms (1.32–1.43 ms)<br>C: 4.23 MiB (4.14–4.36 MiB)<br>S: 4.48 MiB (4.32–4.48 MiB) | 31.45 MiB/s (31.05–79.81 MiB/s)<br>99% RTT: 0.96 ms (0.69–1.00 ms)<br>C: 4.16 MiB (4.15–4.31 MiB)<br>S: 4.32 MiB (4.21–4.51 MiB) | 22.10 MiB/s (12.18–26.82 MiB/s)<br>99% RTT: 1.16 ms (0.94–1.33 ms)<br>C: 4.54 MiB (4.09–4.56 MiB)<br>S: 4.35 MiB (4.31–4.46 MiB) |
| 1 / 800 B / 1% / 10 ms | 0.96 MiB/s (0.96–0.99 MiB/s)<br>99% RTT: 21.18 ms (21.11–31.35 ms)<br>C: 3.76 MiB (3.54–3.79 MiB)<br>S: 4.14 MiB (3.89–4.34 MiB) | 1.00 MiB/s (0.98–1.01 MiB/s)<br>99% RTT: 21.70 ms (21.34–30.65 ms)<br>C: 3.87 MiB (3.79–4.01 MiB)<br>S: 4.27 MiB (4.15–4.29 MiB) | 0.98 MiB/s (0.97–1.01 MiB/s)<br>99% RTT: 31.18 ms (21.26–31.39 ms)<br>C: 3.64 MiB (3.40–3.64 MiB)<br>S: 4.26 MiB (4.07–4.30 MiB) | 1.01 MiB/s (0.92–1.01 MiB/s)<br>99% RTT: 21.34 ms (21.18–31.39 ms)<br>C: 3.77 MiB (3.73–3.97 MiB)<br>S: 4.17 MiB (4.04–4.22 MiB) |
| 64 / 64 B / 0% / 0 ms | 53.40 MiB/s (51.95–54.12 MiB/s)<br>99% RTT: 2.93 ms (2.63–2.96 ms)<br>C: 35.49 MiB (35.43–35.54 MiB)<br>S: 6.01 MiB (5.91–6.04 MiB) | 98.92 MiB/s (95.09–101.14 MiB/s)<br>99% RTT: 1.70 ms (1.62–2.43 ms)<br>C: 35.61 MiB (35.47–35.69 MiB)<br>S: 6.32 MiB (6.18–6.51 MiB) | 44.30 MiB/s (42.14–44.59 MiB/s)<br>99% RTT: 4.96 ms (4.79–5.91 ms)<br>C: 36.84 MiB (36.83–37.10 MiB)<br>S: 6.20 MiB (6.14–6.25 MiB) | 62.60 MiB/s (62.40–62.93 MiB/s)<br>99% RTT: 3.90 ms (3.73–3.99 ms)<br>C: 38.09 MiB (37.93–38.13 MiB)<br>S: 6.59 MiB (6.52–6.77 MiB) |
| 64 / 800 B / 0% / 0 ms | 572.85 MiB/s (564.49–587.24 MiB/s)<br>99% RTT: 3.60 ms (3.39–3.69 ms)<br>C: 36.87 MiB (36.84–36.93 MiB)<br>S: 7.51 MiB (7.34–7.83 MiB) | 586.66 MiB/s (580.31–593.13 MiB/s)<br>99% RTT: 3.18 ms (3.06–3.29 ms)<br>C: 36.96 MiB (36.91–36.97 MiB)<br>S: 7.77 MiB (7.65–7.80 MiB) | 483.01 MiB/s (481.94–485.09 MiB/s)<br>99% RTT: 5.45 ms (5.26–5.55 ms)<br>C: 39.91 MiB (39.83–40.17 MiB)<br>S: 8.21 MiB (8.17–8.23 MiB) | 505.20 MiB/s (500.12–517.84 MiB/s)<br>99% RTT: 4.90 ms (4.87–5.29 ms)<br>C: 40.82 MiB (40.73–41.26 MiB)<br>S: 8.47 MiB (8.14–8.68 MiB) |
| 64 / 800 B / 1% / 0 ms | 499.96 MiB/s (441.68–560.00 MiB/s)<br>99% RTT: 4.35 ms (4.14–5.61 ms)<br>C: 16.93 MiB (16.82–17.07 MiB)<br>S: 7.49 MiB (7.41–8.09 MiB) | 506.62 MiB/s (500.55–525.80 MiB/s)<br>99% RTT: 4.33 ms (4.02–4.94 ms)<br>C: 17.30 MiB (17.04–17.36 MiB)<br>S: 7.77 MiB (7.59–8.02 MiB) | 410.91 MiB/s (378.90–425.06 MiB/s)<br>99% RTT: 5.73 ms (5.34–5.75 ms)<br>C: 20.43 MiB (20.35–20.46 MiB)<br>S: 8.16 MiB (7.98–8.30 MiB) | 436.15 MiB/s (426.11–449.83 MiB/s)<br>99% RTT: 5.54 ms (5.33–5.85 ms)<br>C: 21.32 MiB (21.13–21.35 MiB)<br>S: 8.37 MiB (8.32–8.40 MiB) |
| 128 / 800 B / 0% / 0 ms | 589.86 MiB/s (530.06–590.06 MiB/s)<br>99% RTT: 5.56 ms (5.48–7.78 ms)<br>C: 69.32 MiB (69.20–69.43 MiB)<br>S: 10.69 MiB (10.45–10.77 MiB) | 564.39 MiB/s (552.13–582.06 MiB/s)<br>99% RTT: 6.20 ms (5.50–6.20 ms)<br>C: 69.78 MiB (69.63–69.94 MiB)<br>S: 10.77 MiB (10.50–10.79 MiB) | — | — |
| 512 / 800 B / 0% / 0 ms | 519.76 MiB/s (518.14–539.63 MiB/s)<br>99% RTT: 88.71 ms (29.90–90.20 ms)<br>C: 106.16 MiB (106.02–106.53 MiB)<br>S: 26.50 MiB (25.35–26.54 MiB) | 518.33 MiB/s (515.69–540.13 MiB/s)<br>99% RTT: 84.06 ms (33.42–93.48 ms)<br>C: 107.73 MiB (107.17–108.13 MiB)<br>S: 27.57 MiB (27.50–27.64 MiB) | — | — |

The single-connection bursts contain 300,000 messages at 64 B, 200,000 at
800 B without loss, 50,000 at 4,096 B or 1% loss, 20,000 at 5% loss, and 3,000
for the 10 ms/1% profile. Each concurrent connection sends 30,000 messages,
or 10,000 for the 1% loss and 512-connection profiles.

Blocking costs depend on scheduling and how many messages are ready per call;
a batch call is not a guarantee of higher throughput. Compare latency and
memory as well as throughput before choosing the API. The larger async counts
are a transport echo workload, not a blocking scalability claim. This baseline
uses a different single-connection window and loaded-RTT instrumentation from
the [cross-protocol comparison in the main README](../../README.md#benchmarks), so it is not a controlled version comparison.

[Drivers and reproduction instructions](README.md#async-and-blocking-api-baseline).
