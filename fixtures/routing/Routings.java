/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * The OpenSearch Contributors require contributions made to
 * this file be licensed under the Apache-2.0 license or a
 * compatible open source license.
 */

package org.opensearch.serverless.routing;

import org.opensearch.Version;
import org.opensearch.cluster.metadata.IndexMetadata;
import org.opensearch.cluster.routing.Murmur3HashFunction;
import org.opensearch.cluster.routing.OperationRouting;
import org.opensearch.common.settings.Settings;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Random;

/**
 * Records what the shell's routing classes answer, for the Rust port to check against: {@code RoutingHash},
 * {@code UnitLayout} and {@code UnitLayoutFormat} compiled unchanged, and core's {@code OperationRouting.generateShardId}
 * over the {@code IndexMetadata} the shell renders for an index without a shard mode (routing shards equal to the shard
 * count, as {@code IndexDescriptor.toIndexMetadata} leaves them).
 */
public final class Routings {
    static final int IDS = 10_000;
    static final int[] DECLARED = { 1, 2, 3, 5, 7, 30, 1000, 4096 };

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        final List<String> ids = ids();
        try (PrintWriter out = writer(dir.resolve("hashes.txt"))) {
            for (String id : ids) {
                final long h = RoutingHash.of(id);
                final StringBuilder line = new StringBuilder(hex(id)).append('\t')
                    .append(Long.toHexString(h)).append('\t')
                    .append(Murmur3HashFunction.hash(id));
                for (int n : DECLARED) {
                    line.append('\t').append(RoutingHash.declaredShard(h, n));
                }
                out.println(line);
            }
        }
        try (PrintWriter out = writer(dir.resolve("bounds.txt"))) {
            for (int n : new int[] { 1, 2, 3, 5, 6, 7, 30, 1000 }) {
                for (int s = 0; s < n; s++) {
                    out.println(n + "\t" + s + "\t" + Long.toHexString(RoutingHash.firstHashOf(s, n)) + "\t"
                        + Long.toHexString(RoutingHash.lastHashOf(s, n)));
                }
            }
        }
        legacy(dir, ids);
        layouts(dir, ids);
    }

    // Shard count, partition size; a partitioned index always gets a routing value, as core requires.
    static final int[][] LEGACY = { { 1, 1 }, { 2, 1 }, { 3, 1 }, { 5, 1 }, { 8, 1 }, { 30, 1 }, { 100, 1 }, { 1024, 1 },
        { 5, 2 }, { 8, 3 }, { 30, 7 }, { 100, 99 } };

    static void legacy(Path dir, List<String> ids) throws IOException {
        try (PrintWriter out = writer(dir.resolve("legacy.txt"))) {
            for (int i = 0; i < ids.size(); i++) {
                final int[] config = LEGACY[i % LEGACY.length];
                final IndexMetadata metadata = IndexMetadata.builder("i")
                    .settings(
                        Settings.builder()
                            .put(IndexMetadata.SETTING_VERSION_CREATED, Version.CURRENT)
                            .put(IndexMetadata.SETTING_NUMBER_OF_SHARDS, config[0])
                            .put(IndexMetadata.SETTING_NUMBER_OF_REPLICAS, 0)
                            .put(IndexMetadata.INDEX_ROUTING_PARTITION_SIZE_SETTING.getKey(), config[1])
                    )
                    .build();
                final String id = ids.get(i);
                final String routing = config[1] != 1 || i % 3 == 1 ? ids.get((i * 7 + 3) % ids.size()) : null;
                out.println(config[0] + "\t" + config[1] + "\t" + hex(id) + "\t" + (routing == null ? "~" : hex(routing)) + "\t"
                    + OperationRouting.generateShardId(metadata, id, routing));
            }
        }
    }

    static void layouts(Path dir, List<String> ids) throws IOException {
        final List<UnitLayout> layouts = new ArrayList<>();
        for (int n : new int[] { 1, 2, 3, 5, 7, 64 }) {
            layouts.add(UnitLayout.initial(ShardMode.FIXED, n));
            layouts.add(UnitLayout.initial(ShardMode.ADAPTIVE, n));
        }
        final Random random = new Random(42);
        // Grown layouts: random splits, a few merges, a pending split at the end of some.
        for (int round = 0; round < 24; round++) {
            final int n = new int[] { 1, 3, 5, 7, 12 }[round % 5];
            UnitLayout layout = UnitLayout.initial(round % 2 == 0 ? ShardMode.ADAPTIVE : ShardMode.FIXED, n);
            final int steps = 1 + random.nextInt(14);
            for (int step = 0; step < steps; step++) {
                final List<UnitLayout.Unit> units = layout.units();
                if (random.nextInt(5) == 0 && units.size() > 1) {
                    final int at = random.nextInt(units.size() - 1);
                    try {
                        layout = layout.merge(units.get(at).id(), units.get(at + 1).id());
                    } catch (IllegalArgumentException e) {
                        // refused by the shape rule; the Rust test checks the refusals separately
                    }
                } else {
                    layout = layout.apply(layout.planSplit(units.get(random.nextInt(units.size())).id()));
                }
            }
            if (round % 3 == 0) {
                layout = layout.withPending(layout.planSplit(layout.units().get(random.nextInt(layout.unitCount())).id()));
            }
            layouts.add(layout);
        }
        try (PrintWriter out = writer(dir.resolve("layouts.txt"))) {
            for (UnitLayout layout : layouts) {
                out.println("layout\t" + UnitLayoutFormat.toJson(layout));
                for (UnitLayout.Unit unit : layout.units()) {
                    for (long h : new long[] { unit.first(), unit.last(), unit.first() - 1, unit.last() + 1,
                        unit.first() + ((unit.last() - unit.first()) >>> 1) }) {
                        out.println("unit_for\t" + Long.toHexString(h) + "\t" + layout.unitFor(h).id());
                    }
                    String split;
                    try {
                        final UnitLayout.Split plan = layout.planSplit(unit.id());
                        split = unitText(plan.left()) + "\t" + unitText(plan.right());
                    } catch (IllegalArgumentException | IllegalStateException e) {
                        split = "refused";
                    }
                    out.println("split\t" + unit.id() + "\t" + split);
                }
                for (int i = 0; i < 20; i++) {
                    final long h = random.nextLong();
                    out.println("unit_for\t" + Long.toHexString(h) + "\t" + layout.unitFor(h).id());
                }
                for (int s = 0; s < layout.declaredShards(); s++) {
                    final StringBuilder line = new StringBuilder("units_of\t").append(s);
                    for (UnitLayout.Unit unit : layout.unitsOf(s)) {
                        line.append('\t').append(unit.id()).append(':').append(layout.needsFilter(unit, s));
                    }
                    out.println(line);
                }
                final List<UnitLayout.Unit> units = layout.units();
                for (int at = 0; at + 1 < units.size(); at++) {
                    String merged;
                    try {
                        final UnitLayout after = layout.merge(units.get(at).id(), units.get(at + 1).id());
                        merged = unitText(after.unit(layout.nextUnitId())) + "\t" + after.nextUnitId();
                    } catch (IllegalArgumentException | IllegalStateException e) {
                        merged = "refused";
                    }
                    out.println("merge\t" + units.get(at).id() + "\t" + units.get(at + 1).id() + "\t" + merged);
                }
                if (layout.pending() != null) {
                    out.println("without_pending\t" + UnitLayoutFormat.toJson(layout.withoutPending()));
                    out.println("apply_pending\t" + UnitLayoutFormat.toJson(layout.apply(layout.pending())));
                }
            }
        }
        // Every id routed through three layouts: fixed as created, adaptive grown, fixed with a declared shard halved.
        final UnitLayout fixed = UnitLayout.initial(ShardMode.FIXED, 5);
        final UnitLayout adaptive = layouts.get(layouts.size() - 2).withoutPending();
        UnitLayout halved = UnitLayout.initial(ShardMode.FIXED, 3);
        halved = halved.apply(halved.planSplit(1));
        try (PrintWriter out = writer(dir.resolve("units.txt"))) {
            out.println("fixed\t" + UnitLayoutFormat.toJson(fixed));
            out.println("adaptive\t" + UnitLayoutFormat.toJson(adaptive));
            out.println("halved\t" + UnitLayoutFormat.toJson(halved));
            for (String id : ids) {
                out.println(hex(id) + "\t" + fixed.unitForRouting(id).id() + "\t" + adaptive.unitForRouting(id).id() + "\t"
                    + halved.unitForRouting(id).id());
            }
        }
        refusals(dir);
    }

    static void refusals(Path dir) throws IOException {
        final String[] cases = {
            "{\"mode\":\"fixed\",\"declared_shards\":2,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":1,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"FIXED\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\" adaptive \",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"sideways\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":0,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":1,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":1,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"1\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"fffffffffffffffe\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":0,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":1,\"first\":\"8000000000000001\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"8000000000000000\"},{\"id\":1,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":1,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"},{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":2,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"8fffffffffffffff\"},{\"id\":1,\"first\":\"9000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":3,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"aaaaaaaaaaaaaaaa\"},{\"id\":1,\"first\":\"aaaaaaaaaaaaaaab\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":3,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"aaaaaaaaaaaaaaa9\"},{\"id\":1,\"first\":\"aaaaaaaaaaaaaaaa\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"extra\":{\"a\":[1,2]},\"more\":3}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\",\"note\":\"x\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"10000000000000000\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"xyz\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":-1,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"FFFFFFFFFFFFFFFF\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"00\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":2,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"7fffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":2,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":2,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":1,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":2,\"first\":\"8000000000000001\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":2,\"first\":\"8000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":2,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"8fffffffffffffff\"},{\"id\":2,\"first\":\"9000000000000000\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"7fffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"3fffffffffffffff\"},{\"id\":2,\"first\":\"4000000000000000\",\"last\":\"7fffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":3,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":1,\"first\":\"0\",\"last\":\"ffffffffffffffff\"},{\"id\":2,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"adaptive\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"pending_split\":[]}",
            "[1]",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":{\"id\":0}}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}],\"mode\":\"adaptive\"}",
            "{\"mode\":\"fixed\",\"declared_shards\":\"1\",\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1.0,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":\"fixed\",\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":0,\"last\":\"ffffffffffffffff\"}]}",
            "{\"mode\":null,\"declared_shards\":1,\"next_unit_id\":1,\"units\":[{\"id\":0,\"first\":\"0\",\"last\":\"ffffffffffffffff\"}]}",
        };
        try (PrintWriter out = writer(dir.resolve("refusals.txt"))) {
            for (String json : cases) {
                String answer;
                try {
                    answer = UnitLayoutFormat.toJson(UnitLayoutFormat.fromJson(json));
                } catch (Exception e) {
                    answer = "refused";
                }
                out.println(json + "\t" + answer);
            }
        }
    }

    static List<String> ids() {
        final Random random = new Random(7);
        final List<String> ids = new ArrayList<>(IDS);
        ids.add("");
        ids.add("hello");
        ids.add("1");
        ids.add("é");
        ids.add("日本語");
        ids.add("😀");
        ids.add("x".repeat(300));
        final String alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";
        while (ids.size() < IDS) {
            final int kind = random.nextInt(6);
            final StringBuilder id = new StringBuilder();
            switch (kind) {
                case 0 -> id.append(random.nextInt(1_000_000));
                case 1 -> id.append(new java.util.UUID(random.nextLong(), random.nextLong()));
                case 2 -> {
                    final int len = 1 + random.nextInt(40);
                    for (int i = 0; i < len; i++) {
                        id.append(alphabet.charAt(random.nextInt(alphabet.length())));
                    }
                }
                case 3 -> {
                    final int len = 1 + random.nextInt(8);
                    for (int i = 0; i < len; i++) {
                        id.append((char) (0xA0 + random.nextInt(0x2000)));
                    }
                }
                case 4 -> {
                    final int len = 1 + random.nextInt(4);
                    for (int i = 0; i < len; i++) {
                        id.appendCodePoint(0x10000 + random.nextInt(0x10000));
                    }
                }
                default -> {
                    final int len = 1 + random.nextInt(200);
                    for (int i = 0; i < len; i++) {
                        id.append((char) (0x20 + random.nextInt(0x5F)));
                    }
                }
            }
            ids.add(id.toString());
        }
        return ids;
    }

    static String unitText(UnitLayout.Unit unit) {
        return unit.id() + ":" + Long.toHexString(unit.first()) + ":" + Long.toHexString(unit.last());
    }

    static String hex(String s) {
        return s.isEmpty() ? "-" : HexFormat.of().formatHex(s.getBytes(StandardCharsets.UTF_8));
    }

    static PrintWriter writer(Path path) throws IOException {
        return new PrintWriter(Files.newBufferedWriter(path, StandardCharsets.UTF_8));
    }
}
