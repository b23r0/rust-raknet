#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

/* A small supervisor avoids inheriting a Python runtime before exec. */
static volatile sig_atomic_t child_pid;
static void forward_signal(int signum) {
    int saved_errno = errno;
    if (child_pid > 0) kill((pid_t)child_pid, signum);
    errno = saved_errno;
}

int main(int argc, char **argv) {
    if (argc < 4 || strcmp(argv[2], "--")) {
        fprintf(stderr, "usage: measure_process OUTPUT.json -- COMMAND [ARG ...]\n");
        return 2;
    }
    sigset_t blocked, previous;
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGTERM);
    sigaddset(&blocked, SIGINT);
    if (sigprocmask(SIG_BLOCK, &blocked, &previous)) return 1;
    pid_t child = fork();
    if (child < 0) return 1;
    if (child == 0) {
        sigprocmask(SIG_SETMASK, &previous, NULL);
        execvp(argv[3], argv + 3);
        perror("execvp");
        _exit(127);
    }
    child_pid = child;
    struct sigaction action = {.sa_handler = forward_signal};
    sigemptyset(&action.sa_mask);
    sigaction(SIGTERM, &action, NULL);
    sigaction(SIGINT, &action, NULL);
    sigprocmask(SIG_SETMASK, &previous, NULL);
    int status;
    struct rusage usage;
    while (wait4(child, &status, 0, &usage) < 0) {
        if (errno != EINTR) return 1;
    }
    child_pid = 0;
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : -WTERMSIG(status);
    FILE *output = fopen(argv[1], "w");
    if (!output) return 1;
    fprintf(output, "{\"max_rss_kib\": %ld, \"user_cpu_s\": %.6f, \"system_cpu_s\": %.6f, \"exit_code\": %d}\n",
            usage.ru_maxrss,
            usage.ru_utime.tv_sec + usage.ru_utime.tv_usec / 1e6,
            usage.ru_stime.tv_sec + usage.ru_stime.tv_usec / 1e6, code);
    if (fclose(output)) return 1;
    return code >= 0 ? code : 128 - code;
}
