// Replays the decoder fixtures through the pinned canonical bindings
// (com.google.transit:gtfs-realtime-bindings:0.0.4, as bundled in the
// gtfs-realtime-validator JAR at commit 7041fa3f) and prints what Java does
// with each one.
//
// Its purpose is to record where prost and the canonical decoder disagree.
// crates/gtfs_validator_rt/tests/decoder.rs pins the Rust side; this prints the
// Java side so the two can be compared by eye and any divergence written up as
// an approved delta.
//
//   cargo test -p gtfs-guru-rt --test decoder -- --ignored dump_fixtures
//   java -cp "$GTFS_RT_VALIDATOR_JAR" scripts/rt_parity/decoder_java_check.java \
//        target/rt-decoder-fixtures
//
// Requires a JDK (11+ for the single-file source launcher). Not part of CI.

import com.google.transit.realtime.GtfsRealtime.FeedMessage;
import java.nio.file.*;
import java.util.*;

public class decoder_java_check {
    public static void main(String[] args) throws Exception {
        Path dir = Paths.get(args.length > 0 ? args[0] : "target/rt-decoder-fixtures");
        List<Path> fixtures = new ArrayList<>();
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(dir, "*.pb")) {
            for (Path p : stream) fixtures.add(p);
        }
        Collections.sort(fixtures);

        System.out.printf("%-30s %-10s %s%n", "FIXTURE", "RESULT", "DETAIL");
        System.out.println("-".repeat(96));

        for (Path fixture : fixtures) {
            byte[] bytes = Files.readAllBytes(fixture);
            String name = fixture.getFileName().toString().replace(".pb", "");
            try {
                FeedMessage message = FeedMessage.parseFrom(bytes);
                List<String> notes = new ArrayList<>();
                notes.add("entities=" + message.getEntityCount());
                notes.add("version=\"" + message.getHeader().getGtfsRealtimeVersion() + "\"");

                // Java retains unknown fields; report whether any survived and
                // whether re-serializing reproduces the input byte-for-byte.
                boolean headerUnknown = !message.getHeader().getUnknownFields().asMap().isEmpty();
                boolean anyEntityUnknown = message.getEntityList().stream()
                        .anyMatch(e -> !e.getUnknownFields().asMap().isEmpty());
                if (headerUnknown || anyEntityUnknown) notes.add("unknownFieldsRetained=true");
                notes.add("roundTripIdentical=" + Arrays.equals(message.toByteArray(), bytes));

                // proto2 moves an unrecognised enum value into the unknown-field
                // set, so the field reads as *absent* rather than as present with
                // an invalid value. prost instead surfaces the raw integer.
                notes.add("hasIncrementality=" + message.getHeader().hasIncrementality());

                System.out.printf("%-30s %-10s %s%n", name, "PARSED", String.join(" ", notes));
            } catch (Exception failure) {
                String detail = failure.getMessage();
                if (detail != null && detail.length() > 60) detail = detail.substring(0, 60) + "...";
                System.out.printf("%-30s %-10s %s: %s%n", name, "REJECTED",
                        failure.getClass().getSimpleName(), detail);
            }
        }
    }
}
