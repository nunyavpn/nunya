#import "tunnel_manager.h"

#import <Foundation/Foundation.h>
#import <NetworkExtension/NetworkExtension.h>

// Control plane for the VPN profile: saving it, starting it, talking to the provider.
//
// Nothing here needs privilege. Saving a configuration asks the user once through the system
// prompt; starting the tunnel asks the *system* to launch the extension. The app never touches a
// utun, which is the whole reason for taking this route rather than a privileged helper.
//
// Everything is callback-based. NetworkExtension's completion handlers are delivered through the
// calling thread's run loop, so any attempt to block and wait for one deadlocks — which is exactly
// what an earlier semaphore-based version of this file did.

static void report(nunya_ne_callback cb, void *ctx, int32_t code, NSString *message) {
    if (cb == NULL) {
        return;
    }
    cb(ctx, code, message != nil ? message.UTF8String : NULL);
}

static void report_error(nunya_ne_callback cb, void *ctx, NSError *error, NSString *fallback) {
    NSString *message = error != nil ? error.localizedDescription : fallback;
    report(cb, ctx, NUNYA_NE_ERROR, message ?: @"unknown error");
}

/// Loads the manager this app owns and hands it to `next`, or reports the failure itself.
///
/// loadAllFromPreferences returns every VPN configuration this app has created. We only ever create
/// one, so the first is ours; anything else would be left over from an older version. A nil manager
/// is not an error — it means nothing has been saved yet.
static void with_our_manager(nunya_ne_callback cb,
                             void *ctx,
                             void (^next)(NETunnelProviderManager *manager)) {
    [NETunnelProviderManager loadAllFromPreferencesWithCompletionHandler:^(
        NSArray<NETunnelProviderManager *> *managers, NSError *error) {
        if (error != nil) {
            report_error(cb, ctx, error, nil);
            return;
        }
        next(managers.firstObject);
    }];
}

void nunya_ne_save_configuration(const char *provider_bundle_id,
                                  const char *display_name,
                                  const char *server_address,
                                  const char *config_json,
                                  void *ctx,
                                  nunya_ne_callback cb) {
    @autoreleasepool {
        // Copy out of the C strings now: they belong to the caller and may be freed the moment this
        // returns, long before the blocks below run.
        NSString *bundleId = @(provider_bundle_id);
        NSString *name = @(display_name);
        NSString *server = @(server_address);
        NSString *config = @(config_json);

        with_our_manager(cb, ctx, ^(NETunnelProviderManager *existing) {
            NETunnelProviderManager *manager = existing ?: [[NETunnelProviderManager alloc] init];

            NETunnelProviderProtocol *proto = [[NETunnelProviderProtocol alloc] init];
            // Must match the .appex bundle identifier or the system has nothing to launch.
            proto.providerBundleIdentifier = bundleId;
            // Shown in System Settings, and reported by the provider as the tunnel's remote address.
            proto.serverAddress = server;
            // The extension reads its whole configuration from here.
            proto.providerConfiguration = @{@"config": config};

            manager.protocolConfiguration = proto;
            manager.localizedDescription = name;
            manager.enabled = YES;

            // This is the call that raises "would like to add VPN configurations" the first time.
            [manager saveToPreferencesWithCompletionHandler:^(NSError *saveError) {
                if (saveError != nil) {
                    report_error(cb, ctx, saveError, nil);
                    return;
                }
                // A configuration that was just written has to be read back before its connection
                // object is usable; without this, startVPNTunnel fails as stale.
                [manager loadFromPreferencesWithCompletionHandler:^(NSError *reloadError) {
                    if (reloadError != nil) {
                        report_error(cb, ctx, reloadError, nil);
                        return;
                    }
                    report(cb, ctx, 0, nil);
                }];
            }];
        });
    }
}

void nunya_ne_state_now(void *ctx, nunya_ne_callback cb) {
    @autoreleasepool {
        with_our_manager(cb, ctx, ^(NETunnelProviderManager *manager) {
            // No saved configuration, or the user disabled it in System Settings. Either way the
            // app has to ask before it can connect.
            if (manager == nil || !manager.isEnabled) {
                report(cb, ctx, NUNYA_NE_NEEDS_PERMISSION, nil);
                return;
            }

            nunya_ne_state state;
            switch (manager.connection.status) {
                case NEVPNStatusConnected:
                    state = NUNYA_NE_CONNECTED;
                    break;
                case NEVPNStatusConnecting:
                case NEVPNStatusReasserting:
                    state = NUNYA_NE_CONNECTING;
                    break;
                case NEVPNStatusDisconnecting:
                case NEVPNStatusDisconnected:
                case NEVPNStatusInvalid:
                default:
                    state = NUNYA_NE_DISCONNECTED;
                    break;
            }
            report(cb, ctx, state, nil);
        });
    }
}

void nunya_ne_start(void *ctx, nunya_ne_callback cb) {
    @autoreleasepool {
        with_our_manager(cb, ctx, ^(NETunnelProviderManager *manager) {
            if (manager == nil) {
                report_error(cb, ctx, nil, @"no VPN configuration has been saved yet");
                return;
            }
            if (!manager.isEnabled) {
                report_error(cb, ctx, nil, @"the VPN configuration is disabled in System Settings");
                return;
            }

            NSError *startError = nil;
            // Asks the system to launch the extension. It refuses if the user has not approved the
            // configuration, which is what stops this being a privilege the app grants itself.
            NETunnelProviderSession *session = (NETunnelProviderSession *)manager.connection;
            if (![session startVPNTunnelAndReturnError:&startError]) {
                report_error(cb, ctx, startError, @"could not start the tunnel");
                return;
            }
            report(cb, ctx, 0, nil);
        });
    }
}

void nunya_ne_stop(void *ctx, nunya_ne_callback cb) {
    @autoreleasepool {
        with_our_manager(cb, ctx, ^(NETunnelProviderManager *manager) {
            // Nothing saved means nothing to stop; quitting before a profile exists must not look
            // like a failure.
            if (manager != nil) {
                [manager.connection stopVPNTunnel];
            }
            report(cb, ctx, 0, nil);
        });
    }
}

void nunya_ne_send_message(const char *message, void *ctx, nunya_ne_callback cb) {
    @autoreleasepool {
        NSData *payload = [@(message) dataUsingEncoding:NSUTF8StringEncoding];

        with_our_manager(cb, ctx, ^(NETunnelProviderManager *manager) {
            if (manager == nil) {
                report_error(cb, ctx, nil, @"no VPN configuration has been saved yet");
                return;
            }

            NETunnelProviderSession *session = (NETunnelProviderSession *)manager.connection;
            NSError *sendError = nil;
            BOOL sent = [session sendProviderMessage:payload
                                         returnError:&sendError
                                     responseHandler:^(NSData *response) {
                if (response == nil) {
                    report(cb, ctx, 0, @"");
                    return;
                }
                NSString *text = [[NSString alloc] initWithData:response
                                                       encoding:NSUTF8StringEncoding];
                report(cb, ctx, 0, text ?: @"");
            }];
            if (!sent) {
                report_error(cb, ctx, sendError, @"the provider is not running");
            }
        });
    }
}

void nunya_ne_remove_configuration(void *ctx, nunya_ne_callback cb) {
    @autoreleasepool {
        with_our_manager(cb, ctx, ^(NETunnelProviderManager *manager) {
            if (manager == nil) {
                report(cb, ctx, 0, nil);
                return;
            }
            [manager removeFromPreferencesWithCompletionHandler:^(NSError *error) {
                if (error != nil) {
                    report_error(cb, ctx, error, nil);
                    return;
                }
                report(cb, ctx, 0, nil);
            }];
        });
    }
}
