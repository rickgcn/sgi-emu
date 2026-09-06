#ifndef SE_NETWORK_SLIRP_BRIDGE_H
#define SE_NETWORK_SLIRP_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

typedef struct SeSlirp SeSlirp;
typedef void (*SePacketCallback)(const uint8_t *, size_t, void *);

typedef struct SeSlirpConfig {
    uint32_t network;
    uint32_t mask;
    uint32_t gateway;
    uint32_t dns;
    uint32_t dhcp_start;
} SeSlirpConfig;

SeSlirp *se_slirp_create(const SeSlirpConfig *config, uintptr_t wake_socket,
                        SePacketCallback packet, void *opaque);
void se_slirp_destroy(SeSlirp *session);
int se_slirp_forward(SeSlirp *session, int udp, uint32_t host, uint16_t host_port,
                     uint32_t guest, uint16_t guest_port);
void se_slirp_input(SeSlirp *session, const uint8_t *bytes, size_t length);
int se_slirp_poll(SeSlirp *session, int nonblocking);

#endif
