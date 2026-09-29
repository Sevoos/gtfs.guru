// Replays the header-rule fixtures through the pinned canonical validator
// (MobilityData/gtfs-realtime-validator at commit 7041fa3f) and prints the
// notices its HeaderValidator produces for each one.
//
// Its purpose is to pin canonical E038/E039/E049 behavior for
// crates/gtfs_validator_rt/tests/rules_header.rs. A mismatch is an
// implementation failure unless it is separately reviewed and approved.
//
//   cargo test -p gtfs-guru-rt --test rules_header -- --ignored dump_fixtures
//   java -cp "$GTFS_RT_VALIDATOR_JAR" scripts/rt_parity/header_java_check.java \
//        target/rt-header-fixtures
//
// Each notice prints as <canonical id>[<occurrence prefix>], in the order the
// validator emits them. E049's prefix is empty by design. Expect SLF4J ERROR
// lines on stderr for fixtures whose version does not parse: HeaderValidator
// catches that exception and only logs it, which is the behavior being pinned.
//
// Requires a JDK (11+ for the single-file source launcher). Not part of CI.

import com.google.transit.realtime.GtfsRealtime.FeedMessage;
import edu.usf.cutr.gtfsrtvalidator.lib.model.OccurrenceModel;
import edu.usf.cutr.gtfsrtvalidator.lib.model.helper.ErrorListHelperModel;
import edu.usf.cutr.gtfsrtvalidator.lib.validation.rules.HeaderValidator;
import java.nio.file.*;
import java.util.*;

public class header_java_check {
    public static void main(String[] args) throws Exception {
        Path dir = Paths.get(args.length > 0 ? args[0] : "target/rt-header-fixtures");
        List<Path> fixtures = new ArrayList<>();
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(dir, "*.pb")) {
            for (Path p : stream) fixtures.add(p);
        }
        Collections.sort(fixtures);

        System.out.printf("%-34s %s%n", "FIXTURE", "NOTICES");
        System.out.println("-".repeat(96));

        for (Path fixture : fixtures) {
            String name = fixture.getFileName().toString().replace(".pb", "");
            byte[] bytes = Files.readAllBytes(fixture);
            try {
                FeedMessage message = FeedMessage.parseFrom(bytes);
                // Static data is null: these three rules are current-header and
                // never consult it.
                List<ErrorListHelperModel> errors =
                        new HeaderValidator().validate(0L, null, null, message, null, null);

                List<String> rendered = new ArrayList<>();
                for (ErrorListHelperModel error : errors) {
                    String id = error.getErrorMessage().getValidationRule().getErrorId();
                    for (OccurrenceModel occurrence : error.getOccurrenceList()) {
                        rendered.add(id + "[" + occurrence.getPrefix() + "]");
                    }
                }
                System.out.printf("%-34s %s%n", name,
                        rendered.isEmpty() ? "(none)" : String.join(" ", rendered));
            } catch (Exception failure) {
                System.out.printf("%-34s LOAD FAILED: %s%n", name,
                        failure.getClass().getSimpleName());
            }
        }
    }
}
