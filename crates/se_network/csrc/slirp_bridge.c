#include "slirp_bridge.h"
#include "libslirp.h"
#include <glib.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
typedef WSAPOLLFD SePollFd;
#define se_poll WSAPoll
/* Winsock rejects POLLPRI; TCP urgent data is reported as POLLRDBAND.
 * https://learn.microsoft.com/windows/win32/api/winsock2/nf-winsock2-wsapoll */
#define SE_POLL_READ POLLRDNORM
#define SE_POLL_PRIORITY POLLRDBAND
#else
#include <errno.h>
#include <poll.h>
#include <sys/socket.h>
typedef struct pollfd SePollFd;
#define se_poll poll
#define SE_POLL_READ POLLIN
#define SE_POLL_PRIORITY POLLPRI
#endif

typedef struct SeTimer {
    struct SeTimer *next;
    SlirpTimerCb callback;
    void *opaque;
    int64_t deadline;
} SeTimer;

struct SeSlirp {
    Slirp *slirp;
    SlirpCb callbacks;
    SePacketCallback packet;
    void *opaque;
    slirp_os_socket wake_socket;
    SePollFd *pollfds;
    size_t poll_count;
    size_t poll_capacity;
    SeTimer *timers;
    int failed;
    int notified;
};

static slirp_ssize_t send_packet(const void *bytes, size_t length, void *opaque) {
    SeSlirp *session = opaque;
    session->packet(bytes, length, session->opaque);
    /* A full guest queue deliberately drops the packet, without failing slirp. */
    return (slirp_ssize_t)length;
}

static void guest_error(const char *message, void *opaque) {
    (void)message;
    (void)opaque;
}

static int64_t clock_ns(void *opaque) {
    (void)opaque;
    return g_get_monotonic_time() * INT64_C(1000);
}

static void *timer_new(SlirpTimerCb callback, void *callback_opaque, void *opaque) {
    SeSlirp *session = opaque;
    SeTimer *timer = calloc(1, sizeof(*timer));
    if (!timer) { session->failed = 1; return NULL; }
    timer->callback = callback;
    timer->opaque = callback_opaque;
    timer->deadline = -1;
    timer->next = session->timers;
    session->timers = timer;
    return timer;
}

static void timer_free(void *value, void *opaque) {
    SeSlirp *session = opaque;
    SeTimer **link = &session->timers;
    while (*link && *link != value) link = &(*link)->next;
    if (*link) {
        SeTimer *timer = *link;
        *link = timer->next;
        free(timer);
    }
}

static void timer_mod(void *value, int64_t deadline, void *opaque) {
    (void)opaque;
    if (value) ((SeTimer *)value)->deadline = deadline;
}

static void notify(void *opaque) { ((SeSlirp *)opaque)->notified = 1; }
static void register_socket(slirp_os_socket socket, void *opaque) { (void)socket; (void)opaque; }

static int add_poll(slirp_os_socket socket, int events, void *opaque) {
    SeSlirp *session = opaque;
    if (session->poll_count == session->poll_capacity) {
        size_t capacity = session->poll_capacity ? session->poll_capacity * 2 : 32;
        if (capacity > INT_MAX / sizeof(SePollFd)) { session->failed = 1; return -1; }
        SePollFd *fds = realloc(session->pollfds, capacity * sizeof(*fds));
        if (!fds) { session->failed = 1; return -1; }
        session->pollfds = fds;
        session->poll_capacity = capacity;
    }
    int index = (int)session->poll_count++;
    SePollFd *fd = &session->pollfds[index];
    fd->fd = socket;
    fd->events = (short)(((events & SLIRP_POLL_IN) ? SE_POLL_READ : 0) |
                        ((events & SLIRP_POLL_OUT) ? POLLOUT : 0) |
                        ((events & SLIRP_POLL_PRI) ? SE_POLL_PRIORITY : 0));
    fd->revents = 0;
    return index;
}

static int get_revents(int index, void *opaque) {
    SeSlirp *session = opaque;
    if (index < 0 || (size_t)index >= session->poll_count) return SLIRP_POLL_ERR;
    int revents = session->pollfds[index].revents;
    return ((revents & SE_POLL_READ) ? SLIRP_POLL_IN : 0) |
           ((revents & POLLOUT) ? SLIRP_POLL_OUT : 0) |
           ((revents & SE_POLL_PRIORITY) ? SLIRP_POLL_PRI : 0) |
           ((revents & (POLLERR | POLLNVAL)) ? SLIRP_POLL_ERR : 0) |
           ((revents & POLLHUP) ? SLIRP_POLL_HUP : 0);
}

SeSlirp *se_slirp_create(const SeSlirpConfig *config, uintptr_t wake_socket,
                        SePacketCallback packet, void *opaque) {
    SeSlirp *session = calloc(1, sizeof(*session));
    if (!session) return NULL;
    session->packet = packet;
    session->opaque = opaque;
    session->wake_socket = (slirp_os_socket)wake_socket;
    session->callbacks.send_packet = send_packet;
    session->callbacks.guest_error = guest_error;
    session->callbacks.clock_get_ns = clock_ns;
    session->callbacks.timer_new = timer_new;
    session->callbacks.timer_free = timer_free;
    session->callbacks.timer_mod = timer_mod;
    session->callbacks.notify = notify;
    session->callbacks.register_poll_socket = register_socket;
    session->callbacks.unregister_poll_socket = register_socket;
    SlirpConfig native = {0};
    native.version = 6;
    native.in_enabled = true;
    native.in6_enabled = false;
    native.vnetwork.s_addr = htonl(config->network);
    native.vnetmask.s_addr = htonl(config->mask);
    native.vhost.s_addr = htonl(config->gateway);
    native.vnameserver.s_addr = htonl(config->dns);
    native.vdhcp_start.s_addr = htonl(config->dhcp_start);
    native.if_mtu = 1500;
    native.if_mru = 1500;
    native.enable_emu = false;
    session->slirp = slirp_new(&native, &session->callbacks, session);
    if (!session->slirp || session->failed) {
        se_slirp_destroy(session);
        return NULL;
    }
    return session;
}

void se_slirp_destroy(SeSlirp *session) {
    if (!session) return;
    if (session->slirp) slirp_cleanup(session->slirp);
    while (session->timers) timer_free(session->timers, session);
    free(session->pollfds);
    free(session);
}

int se_slirp_forward(SeSlirp *session, int udp, uint32_t host, uint16_t host_port,
                     uint32_t guest, uint16_t guest_port) {
    struct in_addr host_address, guest_address;
    host_address.s_addr = htonl(host);
    guest_address.s_addr = htonl(guest);
    return slirp_add_hostfwd(session->slirp, udp, host_address, host_port, guest_address, guest_port);
}

void se_slirp_input(SeSlirp *session, const uint8_t *bytes, size_t length) {
    if (length <= INT_MAX) slirp_input(session->slirp, bytes, (int)length);
}

int se_slirp_poll(SeSlirp *session, int nonblocking) {
    uint32_t timeout = nonblocking ? 0 : INT_MAX;
    session->poll_count = 0;
    slirp_pollfds_fill_socket(session->slirp, &timeout, add_poll, session);
    int wake_index = add_poll(session->wake_socket, SLIRP_POLL_IN, session);
    if (session->failed || wake_index < 0) return -1;
    int64_t now = g_get_monotonic_time() / 1000;
    for (SeTimer *timer = session->timers; timer; timer = timer->next) {
        if (timer->deadline >= 0) {
            int64_t delay = timer->deadline > now ? timer->deadline - now : 0;
            if (delay < timeout) timeout = (uint32_t)delay;
        }
    }
    if (session->notified) { timeout = 0; session->notified = 0; }
    int result = se_poll(session->pollfds, (unsigned long)session->poll_count,
                         timeout > INT_MAX ? INT_MAX : (int)timeout);
    if (result < 0) {
#ifdef _WIN32
        if (WSAGetLastError() == WSAEINTR) return 0;
        return WSAGetLastError();
#else
        if (errno == EINTR) return 0;
        return errno;
#endif
    }
    slirp_pollfds_poll(session->slirp, 0, get_revents, session);
    now = g_get_monotonic_time() / 1000;
    /* Callbacks may remove or re-arm timers; restart the traversal each time. */
    for (unsigned int budget = 0; budget < 64; ++budget) {
        SeTimer *due = session->timers;
        while (due && (due->deadline < 0 || due->deadline > now)) due = due->next;
        if (!due) break;
        due->deadline = -1;
        SlirpTimerCb callback = due->callback;
        void *opaque = due->opaque;
        callback(opaque);
    }
    return session->failed ? -1 : 0;
}
