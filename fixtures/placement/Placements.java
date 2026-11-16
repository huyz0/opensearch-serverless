/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * The OpenSearch Contributors require contributions made to
 * this file be licensed under the Apache-2.0 license or a
 * compatible open source license.
 */

package org.opensearch.serverless.cluster;

import org.opensearch.cluster.routing.Murmur3HashFunction;
import org.opensearch.serverless.membership.NodeLease;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Records what the shell's placement classes answer for a fixed fleet, for the Rust port to check against:
 * {@code Rendezvous}, {@code ReaderPlacement}, {@code ZonePreference} (with {@code HotShardSpread} at a rate that
 * spreads to one reader, so the answer is not random) compiled unchanged, and the pure decisions of
 * {@code node/FleetPlacement} copied below, since that class needs the metadata plane to compile.
 */
public final class Placements {
    static final long NOW = 1_700_000_000_000L;
    static final long TTL = 30_000L;

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        final List<NodeLease> fleet = fleet();
        try (PrintWriter out = writer(dir.resolve("fleet.txt"))) {
            for (NodeLease lease : fleet) {
                out.println(lease.toBytes().utf8ToString());
            }
        }
        final String[] strings = {
            "", "a", "ab", "abc", "abcd", "abcde", "logs#0", "n01/logs#0", "é", "日本", "😀x",
            "N3/metrics-2026.11.16#41", "a-very-long-key-that-crosses-several-murmur-blocks/index#2147483647", "z" };
        try (PrintWriter out = writer(dir.resolve("hashes.txt"))) {
            for (String s : strings) {
                out.println(hex(s) + "\t" + Murmur3HashFunction.hash(s) + "\t" + s.hashCode());
            }
        }
        final List<String> ids = new ArrayList<>();
        fleet.forEach(l -> ids.add(l.nodeId()));
        final String[] indexes = { "logs", "metrics-2026.11.16", "événements", "a" };
        try (PrintWriter out = writer(dir.resolve("owners.txt"))) {
            for (String index : indexes) {
                for (int shard : new int[] { 0, 1, 7, 41, 2147483647 }) {
                    for (int size : new int[] { 1, 2, 5, ids.size() }) {
                        final List<String> nodes = ids.subList(0, size);
                        final String key = index + "/" + shard;
                        out.println(key + "\t" + String.join(",", nodes) + "\t" + Rendezvous.owner(key, nodes));
                    }
                }
            }
        }
        try (PrintWriter out = writer(dir.resolve("readers.txt"))) {
            for (String index : indexes) {
                for (int shard : new int[] { 0, 3, 41 }) {
                    for (String role : new String[] { "search", "ingest", "none" }) {
                        for (int limit : new int[] { 0, 1, 2, 4, 50 }) {
                            out.println(index + "\t" + shard + "\t" + role + "\t" + limit + "\t"
                                + String.join(",", ReaderPlacement.candidatesFor(index, shard, fleet, role, limit)));
                        }
                    }
                }
            }
        }
        try (PrintWriter out = writer(dir.resolve("zones.txt"))) {
            for (String index : indexes) {
                for (int shard : new int[] { 0, 3, 41 }) {
                    for (String zone : new String[] { "az-a", "az-b", "az-c", "az-x", "-" }) {
                        for (int requests : new int[] { 0, 1, 20 }) {
                            for (int busy : new int[] { 0, 9 }) {
                                final AtomicLong nanos = new AtomicLong(1_000_000_000_000L);
                                final HotShardSpread spread = new HotShardSpread(50.0, 4, nanos::get);
                                for (int i = 0; i < requests; i++) {
                                    spread.order(index, shard, List.of());
                                }
                                final List<String> ordered = ReaderPlacement.candidatesFor(index, shard, fleet, "search", 2);
                                // Requests already running on placement's first choice: a zone-local reader is
                                // still chosen unless it runs more than the spill bound beyond that.
                                for (String id : ordered.isEmpty() ? List.<String>of() : List.of(ordered.get(0))) {
                                    for (int i = 0; i < busy; i++) {
                                        spread.begin(id);
                                    }
                                }
                                final ZonePreference preference = new ZonePreference(1.0, 8);
                                final String local = zone.equals("-") ? null : zone;
                                final List<String> chosen = preference.order(index, shard, ordered, fleet, "search", local, spread);
                                out.println(index + "\t" + shard + "\t" + zone + "\t" + requests + "\t" + busy + "\t"
                                    + String.join(",", ordered) + "\t" + String.join(",", chosen));
                            }
                        }
                    }
                }
            }
        }
        boundaries(dir);
        try (PrintWriter out = writer(dir.resolve("fleet-decisions.txt"))) {
            for (String local : new String[] { "n01", "n05", "N3" }) {
                for (int held : new int[] { 0, 50, 95 }) {
                    final Fleet f = new Fleet(fleet, local, 100, held, 0.0);
                    final FleetSizeRow size = f.fleetSize();
                    out.println("size\t" + local + "\t" + held + "\t" + size.members + "," + size.wanted + "," + size.demand);
                    out.println("room\t" + local + "\t" + held + "\t" + f.memberWithRoom().orElse("-"));
                    out.println("mean\t" + local + "\t" + held + "\t" + f.fleetMeanHeld());
                    out.println("below\t" + local + "\t" + held + "\t" + f.memberBelowMean().orElse("-"));
                    out.println("handoff\t" + local + "\t" + held + "\t" + f.handOffTarget().orElse("-"));
                    for (String index : indexes) {
                        for (int shard : new int[] { 0, 3, 41 }) {
                            out.println("writer\t" + local + "\t" + held + "\t" + index + "\t" + shard + "\t"
                                + f.writerFor(index, shard).orElse("-"));
                            for (String dead : new String[] { "n02", "n07", "gone-a", "gone-x" }) {
                                for (double over : new double[] { 0.0, 0.5 }) {
                                    final Fleet g = new Fleet(fleet, local, 100, held, over);
                                    out.println("takeover\t" + local + "\t" + held + "\t" + over + "\t" + index + "\t" + shard
                                        + "\t" + dead + "\t" + g.takeoverWinner(index, shard, dead).orElse("-"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /**
     * Small random fleets whose loads are drawn from a few values around each threshold, so that shares, means and
     * room meet their bounds exactly; seeded, so the recording is the same every run.
     */
    static void boundaries(Path dir) throws IOException {
        final java.util.Random random = new java.util.Random(42);
        final int[] loads = { -1, 0, 1, 2, 3, 4, 6, 8 };
        final int[] caps = { -1, 0, 1, 2, 4, 8 };
        final String[] zones = { "az-a", "az-b", null };
        try (PrintWriter out = writer(dir.resolve("boundaries.txt"))) {
            for (int f = 0; f < 400; f++) {
                final List<NodeLease> fleet = new ArrayList<>();
                final StringBuilder spec = new StringBuilder();
                final int size = 1 + random.nextInt(5);
                for (int i = 0; i < size; i++) {
                    final String id = "m" + i;
                    final Set<String> roles = random.nextInt(5) == 0 ? Set.of("search") : Set.of("ingest");
                    final long expires = NOW + (random.nextInt(6) == 0 ? -1 - random.nextInt(2) * TTL : TTL);
                    final String zone = zones[random.nextInt(zones.length)];
                    final int held = loads[random.nextInt(loads.length)];
                    final int cap = caps[random.nextInt(caps.length)];
                    final int inUse = loads[random.nextInt(loads.length)];
                    final int refused = loads[random.nextInt(loads.length)];
                    final boolean revoked = random.nextInt(8) == 0;
                    NodeLease lease = new NodeLease(id, "e" + id, "a" + id, roles, expires);
                    if (zone != null) {
                        lease = lease.withZone(zone);
                    }
                    lease = lease.withLoad(held, cap, inUse, refused);
                    if (revoked) {
                        lease = lease.revokedCopy();
                    }
                    fleet.add(lease);
                    if (spec.length() > 0) {
                        spec.append(';');
                    }
                    spec.append(id).append(':').append(String.join("+", new java.util.TreeSet<>(roles))).append(':')
                        .append(zone == null ? "-" : zone).append(':').append(held).append(':').append(cap).append(':')
                        .append(inUse).append(':').append(refused).append(':').append(expires - NOW).append(':')
                        .append(revoked);
                }
                final String local = random.nextInt(3) == 0 ? "m0" : "self";
                final int held = loads[random.nextInt(loads.length)] * 2;
                final int cap = caps[random.nextInt(caps.length)] * 2;
                final double over = random.nextInt(2) * 0.5;
                final Fleet view = new Fleet(fleet, local, cap, held, over);
                final FleetSizeRow fs = view.fleetSize();
                final String dead = random.nextInt(2) == 0 ? "m" + random.nextInt(size) : "gone-a";
                out.println(spec + "\t" + local + "\t" + held + "\t" + cap + "\t" + over + "\t"
                    + fs.members + "," + fs.wanted + "," + fs.demand + "\t"
                    + view.memberWithRoom().orElse("-") + "\t" + view.fleetMeanHeld() + "\t"
                    + view.memberBelowMean().orElse("-") + "\t" + view.aboveFleetShare() + "\t"
                    + view.handOffTarget().orElse("-") + "\t" + view.writerFor("logs", f).orElse("-") + "\t"
                    + dead + "\t" + view.takeoverWinner("logs", f, dead).orElse("-"));
            }
        }
    }

    /** Twelve members: mixed roles, zones, loads, one revoked, two expired, ids whose order is not by length. */
    static List<NodeLease> fleet() {
        final List<NodeLease> fleet = new ArrayList<>();
        final String[] zones = { "az-a", "az-b", "az-c", null };
        final String[] ids = { "n01", "n02", "n03", "n04", "n05", "n06", "n07", "n08", "n09", "n10", "N3", "z-9" };
        for (int i = 0; i < ids.length; i++) {
            final Set<String> roles = switch (i % 3) {
                case 0 -> Set.of("ingest", "search");
                case 1 -> Set.of("search");
                default -> Set.of("ingest");
            };
            long expires = NOW + TTL;
            if (i == 4) {
                expires = NOW - 1; // expired, inside a lifetime of slack
            }
            if (i == 8) {
                expires = NOW - 2 * TTL; // expired, past the slack
            }
            NodeLease lease = new NodeLease(ids[i], "e" + ids[i], "10.0.0." + i + ":9300", roles, expires);
            final String zone = zones[i % zones.length];
            if (zone != null) {
                lease = lease.withZone(zone);
            }
            if (i != 6) {
                lease = lease.withLoad(10 * i + 3, i == 3 ? 0 : 100, i == 9 ? -1 : 5 * i, i % 4 == 0 ? 2 : -1);
            }
            if (i == 10) {
                lease = lease.revokedCopy();
            }
            fleet.add(lease);
        }
        return fleet;
    }

    record FleetSizeRow(int members, int wanted, long demand) {}

    /** FleetPlacement's decisions over a fixed view, copied from huyz0/OpenSearch node/FleetPlacement.java. */
    static final class Fleet {
        final List<NodeLease> current;
        final String localNodeId;
        final int maxShardsHeld;
        final int heldShards;
        final double takeoverOvercommit;
        final Map<String, String> departedZones = Map.of("gone-a", "az-a");

        Fleet(List<NodeLease> current, String local, int cap, int held, double overcommit) {
            this.current = current;
            this.localNodeId = local;
            this.maxShardsHeld = cap;
            this.heldShards = held;
            this.takeoverOvercommit = overcommit;
        }

        FleetSizeRow fleetSize() {
            int members = 0;
            long demand = 0;
            long caps = 0;
            for (NodeLease lease : current) {
                if (lease.revoked()
                    || NOW >= lease.expiresAtMillis() + TTL
                    || lease.roles().contains("ingest") == false
                    || lease.inUse() < 0
                    || lease.cap() <= 0) {
                    continue;
                }
                members++;
                demand += lease.inUse() + Math.max(0, lease.refused());
                caps += lease.cap();
            }
            if (members == 0) {
                return new FleetSizeRow(0, -1, 0);
            }
            final double perNode = (double) caps / members * 0.8;
            return new FleetSizeRow(members, (int) Math.max(1, Math.ceil(demand / perNode)), demand);
        }

        boolean nearlyFull() {
            return maxShardsHeld > 0 && heldShards >= maxShardsHeld * 0.9;
        }

        Optional<String> memberWithRoom() {
            String best = null;
            double bestShare = 0.75;
            for (NodeLease lease : current) {
                if (lease.nodeId().equals(localNodeId)
                    || lease.isExpiredAt(NOW)
                    || lease.roles().contains("ingest") == false
                    || lease.held() < 0
                    || lease.cap() <= 0) {
                    continue;
                }
                final double share = (double) lease.held() / lease.cap();
                if (share <= bestShare) {
                    best = lease.nodeId();
                    bestShare = share;
                }
            }
            return Optional.ofNullable(best);
        }

        Optional<String> takeoverWinner(String indexName, int shard, String deadOwner) {
            final TreeMap<String, NodeLease> live = new TreeMap<>();
            String deadZone = null;
            for (NodeLease lease : current) {
                if (lease.nodeId().equals(deadOwner)) {
                    deadZone = lease.zone();
                }
                if (lease.nodeId().equals(deadOwner) == false && lease.isExpiredAt(NOW) == false && lease.roles().contains("ingest")) {
                    live.put(lease.nodeId(), lease);
                }
            }
            if (live.isEmpty()) {
                return Optional.empty();
            }
            if (deadZone == null) {
                deadZone = departedZones.get(deadOwner);
            }
            final List<String> inZone = new ArrayList<>();
            for (NodeLease lease : live.values()) {
                if (Zones.same(deadZone, lease.zone())) {
                    inZone.add(lease.nodeId());
                }
            }
            if (inZone.isEmpty() == false) {
                final int firstInZone = Math.floorMod((indexName + "#" + shard).hashCode(), inZone.size());
                for (int i = 0; i < inZone.size(); i++) {
                    final String candidate = inZone.get((firstInZone + i) % inZone.size());
                    if (hasTakeoverRoom(candidate, live.get(candidate))) {
                        return Optional.of(candidate);
                    }
                }
            }
            final List<String> order = new ArrayList<>(live.keySet());
            final int first = Math.floorMod((indexName + "#" + shard).hashCode(), order.size());
            for (int i = 0; i < order.size(); i++) {
                final String candidate = order.get((first + i) % order.size());
                if (hasTakeoverRoom(candidate, live.get(candidate))) {
                    return Optional.of(candidate);
                }
            }
            return Optional.of(order.get(first));
        }

        boolean hasTakeoverRoom(String nodeId, NodeLease lease) {
            if (nodeId.equals(localNodeId)) {
                return maxShardsHeld <= 0 || heldShards < maxShardsHeld + Math.floor(maxShardsHeld * takeoverOvercommit);
            }
            if (lease.held() < 0 || lease.cap() <= 0) {
                return true;
            }
            return lease.held() < lease.cap() + Math.floor(lease.cap() * takeoverOvercommit);
        }

        double fleetMeanHeld() {
            long sum = heldShards;
            int members = 1;
            for (NodeLease lease : current) {
                if (lease.nodeId().equals(localNodeId)
                    || lease.isExpiredAt(NOW)
                    || lease.roles().contains("ingest") == false
                    || lease.held() < 0) {
                    continue;
                }
                sum += lease.held();
                members++;
            }
            return members < 2 ? -1 : (double) sum / members;
        }

        boolean aboveFleetShare() {
            final double mean = fleetMeanHeld();
            return maxShardsHeld > 0 && mean > 0 && heldShards >= maxShardsHeld * 0.25 && heldShards > mean * 1.5;
        }

        Optional<String> memberBelowMean() {
            final double mean = fleetMeanHeld();
            if (mean <= 0) {
                return Optional.empty();
            }
            String best = null;
            long fewest = Long.MAX_VALUE;
            for (NodeLease lease : current) {
                if (lease.nodeId().equals(localNodeId)
                    || lease.isExpiredAt(NOW)
                    || lease.roles().contains("ingest") == false
                    || lease.held() < 0
                    || lease.held() >= mean) {
                    continue;
                }
                if (lease.held() < fewest) {
                    best = lease.nodeId();
                    fewest = lease.held();
                }
            }
            return Optional.ofNullable(best);
        }

        Optional<String> handOffTarget() {
            if (nearlyFull()) {
                final Optional<String> roomy = memberWithRoom();
                if (roomy.isPresent()) {
                    return roomy;
                }
            }
            return aboveFleetShare() ? memberBelowMean() : Optional.empty();
        }

        Optional<String> writerFor(String index, int shard) {
            final List<String> withRoom = new ArrayList<>();
            final List<String> live = new ArrayList<>();
            for (NodeLease lease : current) {
                if (lease.nodeId().equals(localNodeId)
                    || lease.revoked()
                    || lease.isExpiredAt(NOW)
                    || lease.roles().contains("ingest") == false) {
                    continue;
                }
                live.add(lease.nodeId());
                if (lease.cap() <= 0 || lease.held() < 0 || lease.held() < lease.cap()) {
                    withRoom.add(lease.nodeId());
                }
            }
            if (live.isEmpty()) {
                return Optional.empty();
            }
            return Optional.of(Rendezvous.owner(index + "/" + shard, withRoom.isEmpty() ? live : withRoom));
        }
    }

    static String hex(String s) {
        final StringBuilder b = new StringBuilder();
        for (byte x : s.getBytes(StandardCharsets.UTF_8)) {
            b.append(String.format("%02x", x));
        }
        return b.length() == 0 ? "-" : b.toString();
    }

    static PrintWriter writer(Path path) throws IOException {
        return new PrintWriter(Files.newBufferedWriter(path, StandardCharsets.UTF_8));
    }
}
