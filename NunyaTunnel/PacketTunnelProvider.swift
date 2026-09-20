import Foundation
import NetworkExtension
import os.log

// NunyaCore.xcframework, the gomobile build of nunya-core/mobile, installed by scripts/fetch-core.sh.
import NunyaCore

/// The packet tunnel provider: a small, system-launched process that owns the utun and hands its
/// file descriptor to the Go core.
///
/// This is what makes Nunya a normal macOS VPN. The system creates the tunnel interface, so
/// nothing here runs as root, nothing is setuid, and the entry appears in System Settings › VPN
/// beside every other VPN. The app cannot start this by itself — only the system can, and only
/// after the user has approved the configuration.
class PacketTunnelProvider: NEPacketTunnelProvider {
    private let log = Logger(subsystem: "com.nunyavpn.app", category: "provider")
    private var instance: MobileInstance?
    private var platform: PlatformBridge?

    override func startTunnel(options _: [String: NSObject]?) async throws {
        // The app puts the generated sing-box config into providerConfiguration when it saves the
        // VPN profile, so the extension never has to generate one itself.
        guard
            let proto = protocolConfiguration as? NETunnelProviderProtocol,
            let config = proto.providerConfiguration?["config"] as? String,
            !config.isEmpty
        else {
            throw TunnelError.missingConfiguration
        }

        // Must happen before the core starts: the utun does not exist until the system has applied
        // these settings, and openTun would find nothing to hand over.
        try await applyNetworkSettings(proto: proto)

        let bridge = PlatformBridge(log: log)
        let options = MobileStartOptions()
        options.coreConfig = config

        guard let instance = MobileInstance(bridge, options: options) else {
            throw TunnelError.coreStartFailed("could not create the core instance")
        }
        try instance.start()

        self.platform = bridge
        self.instance = instance
        log.info("tunnel started on \(bridge.tunnelName ?? "unknown", privacy: .public)")
    }

    override func stopTunnel(with reason: NEProviderStopReason) async {
        log.info("stopping tunnel: \(reason.rawValue, privacy: .public)")
        try? instance?.close()
        instance = nil
        platform = nil
    }

    /// Answers `sendProviderMessage` from the app. This replaces the unix-socket RPC used on the
    /// other platforms: the extension is not a child of the app and has no socket of its own, so
    /// the system's channel is the only way in.
    override func handleAppMessage(_ messageData: Data) async -> Data? {
        guard let request = try? JSONDecoder().decode(AppMessage.self, from: messageData) else {
            return nil
        }
        switch request.kind {
        case .stats:
            // Tags match config.rs; "proxy" is what the tunnel's traffic leaves through.
            let stats = Stats(
                uplink: instance?.queryOutboundStats("proxy", direction: "uplink") ?? 0,
                downlink: instance?.queryOutboundStats("proxy", direction: "downlink") ?? 0
            )
            return try? JSONEncoder().encode(stats)
        }
    }

    /// Tells the system how to route traffic into the tunnel.
    ///
    /// `includedRoutes` of the default route is the TUN-only promise the UI makes: everything goes
    /// through, so "you're protected" is a claim about the whole device rather than one app.
    private func applyNetworkSettings(proto: NETunnelProviderProtocol) async throws {
        let settings = NEPacketTunnelNetworkSettings(
            tunnelRemoteAddress: proto.serverAddress ?? "127.0.0.1"
        )

        let ipv4 = NEIPv4Settings(addresses: ["172.19.0.1"], subnetMasks: ["255.255.255.0"])
        ipv4.includedRoutes = [NEIPv4Route.default()]
        // Whatever the bypass list resolved to, so it leaves on the physical link. The app computes
        // these; the extension only applies them.
        if let excluded = proto.providerConfiguration?["excludedRoutes"] as? [[String: String]] {
            ipv4.excludedRoutes = excluded.compactMap { entry in
                guard let addr = entry["address"], let mask = entry["mask"] else { return nil }
                return NEIPv4Route(destinationAddress: addr, subnetMask: mask)
            }
        }
        settings.ipv4Settings = ipv4

        let servers = (proto.providerConfiguration?["dnsServers"] as? [String]) ?? ["1.1.1.1"]
        let dns = NEDNSSettings(servers: servers)
        // The core resolves everything itself; claiming all domains stops the system resolver from
        // racing us and leaking queries outside the tunnel.
        dns.matchDomains = [""]
        settings.dnsSettings = dns

        settings.mtu = (proto.providerConfiguration?["mtu"] as? NSNumber) ?? 1500

        try await setTunnelNetworkSettings(settings)
    }

    enum TunnelError: LocalizedError {
        case missingConfiguration
        case tunnelDescriptorUnavailable
        case coreStartFailed(String)

        var errorDescription: String? {
            switch self {
            case .missingConfiguration:
                return "The VPN profile has no configuration attached."
            case .tunnelDescriptorUnavailable:
                return "Could not obtain the tunnel interface from the system."
            case let .coreStartFailed(why):
                return "The core failed to start: \(why)"
            }
        }
    }
}

private struct AppMessage: Decodable {
    enum Kind: String, Decodable { case stats }
    let kind: Kind
}

private struct Stats: Encodable {
    let uplink: Int64
    let downlink: Int64
}

/// Implements the Go core's `PlatformInterface` (the core's `mobile/platform.go`).
///
/// The member that matters is `openTun`: the core asks the platform for a TUN rather than creating
/// one, which is exactly what NetworkExtension requires and the reason no privilege is needed here.
/// This is the same interface the Android build implements in Kotlin.
final class PlatformBridge: NSObject, MobilePlatformInterfaceProtocol {
    private let log: Logger
    private(set) var tunnelName: String?

    init(log: Logger) {
        self.log = log
    }

    /// Hands the core the descriptor of the utun the system already created for us.
    ///
    /// `NEPacketTunnelFlow` exposes no descriptor, so the interface has to be found by scanning our
    /// own open descriptors for the one that is a `utunN` kernel-control socket. This is the
    /// approach WireGuard's Apple clients use, and the core independently validates what it gets
    /// with the matching check in core/mobile/sys_darwin.go.
    func openTun(_: MobileTunOptionsProtocol?, ret0_: UnsafeMutablePointer<Int32>?) throws {
        guard let (fd, name) = Self.findTunnelDescriptor() else {
            throw PacketTunnelProvider.TunnelError.tunnelDescriptorUnavailable
        }
        tunnelName = name
        log.info("handing \(name, privacy: .public) (fd \(fd, privacy: .public)) to the core")
        ret0_?.pointee = fd
    }

    /// Returns the descriptor and name of this process's utun, if it has one.
    private static func findTunnelDescriptor() -> (Int32, String)? {
        // The system opens the utun before the provider runs, so it sits low in the table; scanning
        // a bounded range avoids walking every possible descriptor.
        for fd in Int32(0) ... Int32(1024) {
            var addr = sockaddr_ctl()
            var len = socklen_t(MemoryLayout<sockaddr_ctl>.size)
            let isControlSocket = withUnsafeMutablePointer(to: &addr) { pointer -> Bool in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                    getpeername(fd, sa, &len) == 0
                }
            }
            guard isControlSocket, addr.sc_family == UInt8(AF_SYSTEM) else { continue }

            var name = [CChar](repeating: 0, count: Int(IFNAMSIZ))
            var nameLen = socklen_t(name.count)
            // UTUN_OPT_IFNAME is 2 in <net/if_utun.h>, which Swift does not import.
            guard getsockopt(fd, SYSPROTO_CONTROL, 2, &name, &nameLen) == 0 else { continue }

            let interface = String(cString: name)
            if interface.hasPrefix("utun") {
                return (fd, interface)
            }
        }
        return nil
    }

    // The rest of the protocol exists because the Go interface requires it. The system handles
    // interface selection and process attribution for us, so most of these are deliberately inert.

    func usePlatformAutoDetectControl() -> Bool { false }

    func autoDetectControl(_: Int32) throws {}

    func useProcFS() -> Bool { false }

    func findConnectionOwner(
        _: Int32, sourceAddress _: String?, sourcePort _: Int32,
        destinationAddress _: String?, destinationPort _: Int32
    ) throws -> MobileConnectionOwner {
        throw PacketTunnelProvider.TunnelError.coreStartFailed("process lookup is not supported")
    }

    func startDefaultInterfaceMonitor(_: MobileInterfaceUpdateListenerProtocol?) throws {}

    func closeDefaultInterfaceMonitor(_: MobileInterfaceUpdateListenerProtocol?) throws {}

    func getInterfaces() throws -> MobileNetworkInterfaceIteratorProtocol {
        throw PacketTunnelProvider.TunnelError.coreStartFailed(
            "interface enumeration is not supported"
        )
    }

    func readWIFIState() -> MobileWIFIState? { nil }

    func clearDNSCache() {}

    func localDNSTransport() -> MobileLocalDNSTransportProtocol? { nil }

    func send(_: MobileNotification?) throws {}

    func cancelNotification(_: String?, typeID _: Int32) throws {}
}
