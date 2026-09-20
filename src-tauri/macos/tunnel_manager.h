// C ABI over NETunnelProviderManager, for the Rust side.
//
// Every call is asynchronous and takes a callback. An earlier version blocked on a dispatch
// semaphore to present a synchronous API, which deadlocked: NetworkExtension's completion handlers
// do not fire unless the calling thread is servicing its run loop, so waiting on that thread waits
// forever. Handing the result back through a callback removes the dependency entirely — the
// handler fires on whatever queue NetworkExtension chooses, and Rust resolves a channel.
//
// `ctx` is passed straight back to the callback and is never dereferenced here.
// `message` is only valid for the duration of the callback; copy it if you need it.

#ifndef NUNYA_TUNNEL_MANAGER_H
#define NUNYA_TUNNEL_MANAGER_H

#include <stdint.h>

// Mirrors transport::TunnelState.
typedef enum {
    NUNYA_NE_DISCONNECTED = 0,
    NUNYA_NE_CONNECTING = 1,
    NUNYA_NE_CONNECTED = 2,
    // No VPN configuration has been saved and approved yet.
    NUNYA_NE_NEEDS_PERMISSION = 3,
    // Failed; `message` holds why.
    NUNYA_NE_ERROR = -1,
} nunya_ne_state;

// For calls that only succeed or fail, `code` is 0 or NUNYA_NE_ERROR. For state queries it is a
// nunya_ne_state. For send_message, 0 means `message` holds the provider's reply.
typedef void (*nunya_ne_callback)(void *ctx, int32_t code, const char *message);

// Saves (creating if needed) this app's VPN configuration.
//
// The first call raises the system's "would like to add VPN configurations" prompt and, once
// approved, puts the entry in System Settings > VPN. `config_json` is the generated sing-box
// configuration, carried in providerConfiguration so the extension never generates one itself.
void nunya_ne_save_configuration(const char *provider_bundle_id,
                                  const char *display_name,
                                  const char *server_address,
                                  const char *config_json,
                                  void *ctx,
                                  nunya_ne_callback cb);

// Current state of our configuration, without changing anything.
void nunya_ne_state_now(void *ctx, nunya_ne_callback cb);

// Asks the system to start the tunnel. The extension is launched by the system, never by us.
void nunya_ne_start(void *ctx, nunya_ne_callback cb);

void nunya_ne_stop(void *ctx, nunya_ne_callback cb);

// Sends a JSON message to the running provider; the reply arrives as the callback's `message`.
void nunya_ne_send_message(const char *message, void *ctx, nunya_ne_callback cb);

// Removes the VPN configuration entirely, so the entry disappears from System Settings.
void nunya_ne_remove_configuration(void *ctx, nunya_ne_callback cb);

#endif
