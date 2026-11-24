package org.rustgis.mobile;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;

/**
 * Writes a project over a document opened for reading and writing. Android's document API has no
 * way to replace a document whole, so the bytes are written from its start, the rest cut off, and
 * the result forced to storage; the caller keeps a complete copy of its own until this returns.
 */
final class DocumentWrite {
    /**
     * Why a write-back failed, as codes the Rust side words in the user's language (shell.rs,
     * WriteBackFailed); any other reason is the system's own text.
     */
    static final String BUSY = "code:busy";
    static final String STORAGE = "code:storage";
    static final String UNWRITABLE = "code:unwritable";
    static final String STALLED = "code:stalled";
    static final String SHORT = "code:short";
    static final String UNFINISHED = "code:unfinished";

    private DocumentWrite() {}

    /**
     * Writes {@code bytes} over everything {@code channel} held, then forces it to storage: a
     * document longer before is cut to the new length.
     */
    static void overwrite(FileChannel channel, byte[] bytes) throws IOException {
        ByteBuffer buffer = ByteBuffer.wrap(bytes);
        long at = 0;
        while (buffer.hasRemaining()) {
            int n = channel.write(buffer, at);
            if (n <= 0) {
                throw new IOException(STALLED);
            }
            at += n;
        }
        channel.truncate(bytes.length);
        channel.force(true);
        long size = channel.size();
        if (size != bytes.length) {
            throw new IOException(SHORT);
        }
    }
}
