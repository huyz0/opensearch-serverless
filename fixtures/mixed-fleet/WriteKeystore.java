import org.opensearch.common.settings.KeyStoreWrapper;

import java.nio.file.Files;
import java.nio.file.Path;

/** Writes an empty-password opensearch.keystore holding the S3 credentials: {@code WriteKeystore <home> <access> <secret>}. */
public final class WriteKeystore {
    public static void main(String[] args) throws Exception {
        final Path config = Path.of(args[0]).resolve("config");
        Files.createDirectories(config);
        final KeyStoreWrapper keystore = KeyStoreWrapper.create();
        keystore.setString("s3.client.default.access_key", args[1].toCharArray());
        keystore.setString("s3.client.default.secret_key", args[2].toCharArray());
        keystore.save(config, new char[0]);
    }
}
