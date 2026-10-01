#define _GNU_SOURCE
#define main single_benchmark_main
#include "benchmark.c"
#undef main
#include <sys/epoll.h>
#include <pthread.h>

typedef struct { Session io; size_t id, sent, received; uint64_t sent_at; } Peer;
static _Thread_local size_t total_peers, payload_size, messages_per_peer;
static _Thread_local int is_server, measured;
static _Thread_local Peer *peers;
static _Thread_local char *message_buffer, *receive_buffer;
static _Thread_local size_t completed, rtt_count;
static _Thread_local double *rtts;
static _Thread_local size_t id_offset, worker_index;
static pthread_barrier_t ready_barrier, finish_barrier;
typedef struct { uint64_t start, end; double setup; double *latency; size_t samples; } WorkerResult;
static WorkerResult results[4];

static void make_payload(size_t id, size_t message) {
    for (size_t i=0;i<8;++i) {
        message_buffer[1+i]=(char)(((uint64_t)id>>(i*8))&255);
        message_buffer[9+i]=(char)(((uint64_t)message>>(i*8))&255);
    }
}
static void setup_peer(Peer *p, int fd, uint32_t conv) {
    p->io.fd=fd; p->io.kcp=ikcp_create(conv,&p->io); if (!p->io.kcp) exit(1);
    p->io.kcp->output=output;
    ikcp_setmtu(p->io.kcp,1400);ikcp_wndsize(p->io.kcp,64,128);ikcp_nodelay(p->io.kcp,1,10,2,1);
    ikcp_update(p->io.kcp,(uint32_t)(nanos()/1000000ULL));
}
static void fill(Peer *p) {
    size_t maximum=measured?messages_per_peer:20;
    size_t pipeline=measured?16:1;
    while (p->sent<maximum && p->sent-p->received<pipeline && ikcp_waitsnd(p->io.kcp)<64) {
        make_payload(p->id,p->sent+(measured?20:0));
        p->sent_at=nanos();send_record(&p->io,message_buffer,payload_size);++p->sent;
    }
}
static void drain(Peer *p) {
    for (;;) {
        if (is_server && ikcp_waitsnd(p->io.kcp)>=64) break;
        int n=receive_record(&p->io,receive_buffer,payload_size);if(n<0)break;
        if(is_server) {send_record(&p->io,receive_buffer,(size_t)n);continue;}
        make_payload(p->id,p->received+(measured?20:0));
        if(n!=(int)payload_size || memcmp(receive_buffer,message_buffer,payload_size)) {fprintf(stderr,"Ordered echo mismatch\n");exit(1);}
        if(!measured)rtts[rtt_count++]=(nanos()-p->sent_at)/1000.0;
        ++p->received;
        if(p->received==(measured?messages_per_peer:20))++completed;
    }
    if(!is_server)fill(p);
}
static int worker_main(int argc,char **argv) {
    if(argc!=6){fprintf(stderr,"usage: kcp_concurrency server|client ADDRESS CONNECTIONS MESSAGES PAYLOAD\n");return 2;}
    is_server=!strcmp(argv[1],"server");
    if(!is_server && strcmp(argv[1],"client"))return 2;
    total_peers=number(argv[3]);messages_per_peer=number(argv[4]);payload_size=number(argv[5]);
    if(!total_peers || total_peers>8192 || !messages_per_peer || payload_size<17 || payload_size>65536)return 2;
    char ip[64];const char *colon=strrchr(argv[2],':');if(!colon || (size_t)(colon-argv[2])>=sizeof(ip))return 2;
    memcpy(ip,argv[2],(size_t)(colon-argv[2]));ip[colon-argv[2]]=0;
    struct sockaddr_in address={.sin_family=AF_INET,.sin_port=htons((uint16_t)number(colon+1))};if(inet_pton(AF_INET,ip,&address.sin_addr)!=1)return 2;
    peers=calloc(total_peers,sizeof(*peers));message_buffer=malloc(payload_size);receive_buffer=malloc(payload_size);rtts=malloc(total_peers*20*sizeof(double));
    if(!peers || !message_buffer || !receive_buffer || !rtts)return 1;
    memset(message_buffer, 0xfe, payload_size);
    int epoll=epoll_create1(EPOLL_CLOEXEC);if(epoll<0)fail("epoll_create");
    int *ports=calloc(65536,sizeof(int));if(!ports)return 1;
    size_t established=0;int server_fd=-1;
    uint64_t setup_start=nanos();
    for(size_t i=0;i<(is_server?1:total_peers);++i) {
        int fd=socket(AF_INET,SOCK_DGRAM,0);if(fd<0)fail("socket");int buffer=2*1024*1024;
        if(setsockopt(fd,SOL_SOCKET,SO_RCVBUF,&buffer,sizeof(buffer)))fail("SO_RCVBUF");
        if(fcntl(fd,F_SETFL,O_NONBLOCK))fail("fcntl");
        if(is_server){int reuse=1;if(setsockopt(fd,SOL_SOCKET,SO_REUSEPORT,&reuse,sizeof(reuse)))fail("SO_REUSEPORT");if(bind(fd,(struct sockaddr *)&address,sizeof(address)))fail("bind");server_fd=fd;}
        else {peers[i].id=i+id_offset;peers[i].io.peer=address;peers[i].io.known_peer=1;setup_peer(&peers[i],fd,42+(uint32_t)(i+id_offset));++established;}
        struct epoll_event event={.events=EPOLLIN,.data.u32=(uint32_t)i};if(epoll_ctl(epoll,EPOLL_CTL_ADD,fd,&event))fail("epoll_ctl");
    }
    double setup_seconds=(nanos()-setup_start)/1000000000.0;
    if(is_server){printf("Official C KCP multi-peer echo listening\n");fflush(stdout);}
    else for(size_t i=0;i<total_peers;++i)fill(&peers[i]);
    uint64_t last_timer=nanos(),burst_start=0;
    for(;;) {
        struct epoll_event events[256];int n=epoll_wait(epoll,events,256,1);if(n<0 && errno!=EINTR)fail("epoll_wait");
        for(int e=0;e<n;++e) {
            int fd=is_server?server_fd:peers[events[e].data.u32].io.fd;
            for(int batch=0;batch<256;++batch) {
                char packet[65536];struct sockaddr_in from;socklen_t len=sizeof(from);
                ssize_t bytes=recvfrom(fd,packet,sizeof(packet),0,(struct sockaddr *)&from,&len);
                if(bytes<0){if(errno==EAGAIN || errno==EWOULDBLOCK)break;if(errno==EINTR)continue;fail("recvfrom");}
                if(bytes<24)continue;
                Peer *p;
                if(is_server) {
                    unsigned port=ntohs(from.sin_port);
                    if(!ports[port]) {
                        if(established==total_peers){fprintf(stderr,"Too many peers\n");return 1;}
                        p=&peers[established];p->id=established;p->io.peer=from;p->io.known_peer=1;
                        setup_peer(p,server_fd,ikcp_getconv(packet));ports[port]=(int)++established;
                    }
                    p=&peers[ports[port]-1];
                    if(p->io.peer.sin_addr.s_addr!=from.sin_addr.s_addr)continue;
                } else p=&peers[events[e].data.u32];
                if(ikcp_input(p->io.kcp,packet,bytes)<0){fprintf(stderr,"Invalid KCP packet\n");return 1;}
                ikcp_update(p->io.kcp,(uint32_t)(nanos()/1000000ULL));ikcp_flush(p->io.kcp);drain(p);
            }
        }
        uint64_t now=nanos();
        if(now-last_timer>=10000000ULL) {
            for(size_t i=0;i<established;++i){ikcp_update(peers[i].io.kcp,(uint32_t)(now/1000000ULL));drain(&peers[i]);}
            last_timer=now;
        }
        if(!is_server && completed==total_peers) {
            if(!measured){
                pthread_barrier_wait(&ready_barrier);
                measured=1;completed=0;burst_start=nanos();
                for(size_t i=0;i<total_peers;++i){peers[i].sent=peers[i].received=0;fill(&peers[i]);}
            } else {
                results[worker_index]=(WorkerResult){.start=burst_start,.end=nanos(),.setup=setup_seconds,.latency=rtts,.samples=rtt_count};
                pthread_barrier_wait(&finish_barrier);
                break;
            }
        }
    }
    for(size_t i=0;i<established;++i){ikcp_release(peers[i].io.kcp);if(!is_server)close(peers[i].io.fd);}
    free(ports);free(peers);free(message_buffer);free(receive_buffer);close(epoll);return 0;
}

typedef struct { int argc; char **argv; size_t index, offset; } WorkerArgs;
static void *run_worker(void *arg) {
    WorkerArgs *a=arg;worker_index=a->index;id_offset=a->offset;
    int rc=worker_main(a->argc,a->argv);if(rc)exit(rc);
    return NULL;
}
int main(int argc,char **argv) {
    if(argc!=6)return 2;
    size_t n=number(argv[3]),messages=number(argv[4]),size=number(argv[5]);
    if(n<4 || n>8192 || !messages || size<17 || size>65536)return 2;
    int server=!strcmp(argv[1],"server");
    pthread_barrier_init(&ready_barrier,NULL,4);pthread_barrier_init(&finish_barrier,NULL,4);
    pthread_t workers[4];WorkerArgs args[4];char counts[4][32];char *options[4][6];
    size_t offset=0;
    for(size_t i=0;i<4;++i) {
        size_t count=server?n:n/4+(i<n%4);
        snprintf(counts[i],sizeof(counts[i]),"%zu",count);
        for (int j = 0; j < 6; ++j) {
            options[i][j] = argv[j];
        }
        options[i][3] = counts[i];
        args[i]=(WorkerArgs){.argc=argc,.argv=options[i],.index=i,.offset=offset};offset+=count;
        if(pthread_create(&workers[i],NULL,run_worker,&args[i]))fail("pthread_create");
    }
    for(size_t i=0;i<4;++i)pthread_join(workers[i],NULL);
    uint64_t start=UINT64_MAX,end=0;double setup=0;size_t sample_count=0;
    for(size_t i=0;i<4;++i){if(results[i].start<start)start=results[i].start;if(results[i].end>end)end=results[i].end;if(results[i].setup>setup)setup=results[i].setup;sample_count+=results[i].samples;}
    double *values=malloc(sample_count*sizeof(double));if(!values)return 1;offset=0;
    for(size_t i=0;i<4;++i){memcpy(values+offset,results[i].latency,results[i].samples*sizeof(double));offset+=results[i].samples;free(results[i].latency);}
    qsort(values,sample_count,sizeof(double),cmp);double elapsed=(end-start)/1000000000.0;
    printf("Connections: %zu\nMessages per connection: %zu\nPayload size: %zu bytes\nSetup: %.6f s\nElapsed: %.6f s\n",n,messages,size,setup,elapsed);
    printf("Echo payload throughput (per direction): %.2f MiB/s\n",(double)n*messages*size/1048576.0/elapsed);
    printf("RTT p50: %.1f us\nRTT p95: %.1f us\nRTT p99: %.1f us\n",percentile(values,sample_count,50),percentile(values,sample_count,95),percentile(values,sample_count,99));
    printf("Verified ordered echoes: %zu\n",n*(messages+20));free(values);return 0;
}
