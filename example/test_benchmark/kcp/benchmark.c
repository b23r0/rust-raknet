#define _POSIX_C_SOURCE 200809L
#include "ikcp.h"
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

typedef struct {
    int fd, known_peer;
    struct sockaddr_in peer;
    ikcpcb *kcp;
} Session;

static uint64_t nanos(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) { perror("clock_gettime"); exit(1); }
    return (uint64_t)t.tv_sec * 1000000000ULL + (uint64_t)t.tv_nsec;
}
static void fail(const char *s) { perror(s); exit(1); }
static int output(const char *buf, int len, ikcpcb *kcp, void *user) {
    (void)kcp;
    Session *s = user;
    ssize_t n = sendto(s->fd, buf, (size_t)len, 0, (struct sockaddr *)&s->peer, sizeof(s->peer));
    if (n < 0 && errno != EAGAIN && errno != EWOULDBLOCK) fail("sendto");
    /* Socket pressure is treated as packet loss; KCP retransmits reliably. */
    return n == len ? 0 : -1;
}
static void pump(Session *s) {
    uint32_t now = (uint32_t)(nanos() / 1000000ULL);
    ikcp_update(s->kcp, now);
    int timeout = (int32_t)(ikcp_check(s->kcp, now) - now);
    if (timeout < 0) timeout = 0;
    if (timeout > 10) timeout = 10;
    struct pollfd fd = { .fd = s->fd, .events = POLLIN };
    if (poll(&fd, 1, timeout) < 0 && errno != EINTR) fail("poll");
    for (int i = 0; i < 256; ++i) {
        char buf[65536]; struct sockaddr_in from; socklen_t len = sizeof(from);
        ssize_t n = recvfrom(s->fd, buf, sizeof(buf), 0, (struct sockaddr *)&from, &len);
        if (n < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) break;
            if (errno == EINTR) continue;
            fail("recvfrom");
        }
        if (!s->known_peer) { s->peer = from; s->known_peer = 1; }
        if (from.sin_addr.s_addr != s->peer.sin_addr.s_addr || from.sin_port != s->peer.sin_port) continue;
        if (ikcp_input(s->kcp, buf, n) < 0) { fprintf(stderr, "Invalid KCP packet\n"); exit(1); }
        /* Flush ACKs after each datagram, including out-of-order segments. */
        ikcp_update(s->kcp, (uint32_t)(nanos() / 1000000ULL));
        ikcp_flush(s->kcp);
    }
    ikcp_update(s->kcp, (uint32_t)(nanos() / 1000000ULL));
    ikcp_flush(s->kcp);
}
static void send_record(Session *s, const char *buf, size_t len) {
    if (ikcp_send(s->kcp, buf, (int)len) < 0) { fprintf(stderr, "KCP send failed\n"); exit(1); }
    ikcp_update(s->kcp, (uint32_t)(nanos() / 1000000ULL));
    ikcp_flush(s->kcp);
}
static int receive_record(Session *s, char *buf, size_t size) {
    int n = ikcp_recv(s->kcp, buf, (int)size);
    if (n == -3) { fprintf(stderr, "Oversized KCP record\n"); exit(1); }
    return n;
}
static void verify(const char *buf, int n, const char *expected, size_t size) {
    if (n != (int)size) { fprintf(stderr, "Echo size mismatch\n"); exit(1); }
    if (memcmp(buf, expected, size)) { fprintf(stderr, "Echo data mismatch\n"); exit(1); }
}
static double round_trip(Session *s, const char *payload, char *response, size_t size) {
    uint64_t start = nanos(); send_record(s, payload, size);
    for (;;) { int n = receive_record(s, response, size); if (n >= 0) { verify(response, n, payload, size); break; } pump(s); }
    return (nanos() - start) / 1000.0;
}
static int cmp(const void *a, const void *b) { double x = *(const double *)a, y = *(const double *)b; return (x > y) - (x < y); }
static double percentile(double *v, size_t n, size_t p) { return v[(n * p + 99) / 100 - 1]; }
static size_t number(const char *s) {
    char *end; errno = 0; unsigned long n = strtoul(s, &end, 10);
    if (errno || *end || *s == '-') { fprintf(stderr, "Invalid number\n"); exit(2); }
    return (size_t)n;
}
int main(int argc, char **argv) {
    const char *mode = NULL, *address = NULL;
    size_t packets = 10000, size = 800, warmup = 100, samples = 300, window = 64;
    for (int i = 1; i < argc; i += 2) {
        if (i + 1 >= argc) { fprintf(stderr, "Missing option value\n"); return 2; }
        const char *key = argv[i], *value = argv[i+1];
        if (!strcmp(key, "--type")) mode = value;
        else if (!strcmp(key, "--address")) address = value;
        else if (!strcmp(key, "--packets")) packets = number(value);
        else if (!strcmp(key, "--payload-size")) size = number(value);
        else if (!strcmp(key, "--warmup")) warmup = number(value);
        else if (!strcmp(key, "--latency-samples")) samples = number(value);
        else if (!strcmp(key, "--kcp-window")) window = number(value);
        else if (!strcmp(key, "--protocol") && !strcmp(value, "kcp")) {}
        else { fprintf(stderr, "Unknown option\n"); return 2; }
    }
    if (!mode || !address || (strcmp(mode,"server") && strcmp(mode,"client")) || !size || size > 65536 || !packets || !window || window > 65535) return 2;
    char ip[64]; const char *colon = strrchr(address, ':');
    if (!colon || (size_t)(colon-address) >= sizeof(ip)) return 2;
    memcpy(ip,address,(size_t)(colon-address)); ip[colon-address] = 0;
    size_t port = number(colon+1); if (!port || port > 65535) return 2;
    struct sockaddr_in addr = { .sin_family = AF_INET, .sin_port = htons((uint16_t)port) };
    if (inet_pton(AF_INET,ip,&addr.sin_addr) != 1) return 2;
    Session s = { .fd = socket(AF_INET, SOCK_DGRAM, 0) }; if (s.fd < 0) fail("socket");
    int buffer = 2 * 1024 * 1024;
    if (setsockopt(s.fd,SOL_SOCKET,SO_RCVBUF,&buffer,sizeof(buffer))) fail("SO_RCVBUF");
    if (fcntl(s.fd,F_SETFL,O_NONBLOCK)) fail("fcntl");
    int server = !strcmp(mode,"server");
    if (server) { if (bind(s.fd,(struct sockaddr *)&addr,sizeof(addr))) fail("bind"); }
    else { s.peer = addr; s.known_peer = 1; }
    s.kcp = ikcp_create(42,&s); if (!s.kcp) return 1;
    s.kcp->output = output;
    ikcp_setmtu(s.kcp,1400); ikcp_wndsize(s.kcp,(int)window,(int)window); ikcp_nodelay(s.kcp,1,10,2,1);
    ikcp_update(s.kcp,(uint32_t)(nanos()/1000000ULL));
    char *payload = malloc(size), *response = malloc(size); if (!payload || !response) return 1;
    memset(payload,0xfe,size);
    if (server) {
        printf("Official C KCP echo listening on %s\n",address); fflush(stdout);
        for (;;) {
            pump(&s);
            while (ikcp_waitsnd(s.kcp) < (int)window) {
                int n = receive_record(&s,response,size); if (n < 0) break;
                send_record(&s,response,(size_t)n);
            }
        }
    }
    for (size_t i = 0; i < warmup; ++i) round_trip(&s,payload,response,size);
    double *latency = samples ? malloc(samples*sizeof(double)) : NULL;
    if (samples && !latency) return 1;
    for (size_t i = 0; i < samples; ++i) latency[i] = round_trip(&s,payload,response,size);
    uint64_t start = nanos(); size_t sent = 0, received = 0;
    while (received < packets) {
        /* Consume ready echoes before refilling their slots. Polling first
           would add an idle timer wait after each completed application batch. */
        for (;;) { int n = receive_record(&s,response,size); if (n < 0) break; verify(response,n,payload,size); ++received; }
        while (sent < packets && sent - received < window && ikcp_waitsnd(s.kcp) < (int)window) { send_record(&s,payload,size); ++sent; }
        if (received < packets) pump(&s);
    }
    double elapsed = (nanos()-start)/1000000000.0;
    printf("Protocol: C KCP\nPackets: %zu\nPayload size: %zu bytes\nWarmup rounds: %zu\nRTT samples: %zu\nElapsed: %.6f s\n",packets,size,warmup,samples,elapsed);
    printf("Echo payload throughput (per direction): %.2f MiB/s\n",(double)packets*size/1048576.0/elapsed);
    printf("KCP: MTU 1400; window %zu; nodelay 1/10/2/1; immediate writes and ACKs; message mode\n",window);
    if (samples) {
        qsort(latency,samples,sizeof(double),cmp);
        printf("RTT p50: %.1f us\nRTT p95: %.1f us\nRTT p99: %.1f us\n",percentile(latency,samples,50),percentile(latency,samples,95),percentile(latency,samples,99));
    }
    ikcp_release(s.kcp); close(s.fd); free(payload); free(response); free(latency); return 0;
}
