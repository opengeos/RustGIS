package org.rustgis.mobile;

/** Host-JVM regression checks; run with assertions enabled (no Android runtime needed). */
public final class PhoneContractsTest {
    public static void main(String[] args) {
        EventBuffer events = new EventBuffer(1200);
        events.add("picked\tdata\told\t/file");
        for (int i = 0; i < 2000; i++) {
            events.add("position\t" + i);
        }
        events.add("ended\tdxf:7");
        int fixes = 0;
        int notices = 0;
        int completions = 0;
        int previous = 799;
        while (!events.isEmpty()) {
            String[] batch = events.take().split("\n");
            assert batch.length <= EventBuffer.PER_FRAME;
            for (String event : batch) {
                if (event.startsWith("position\t")) {
                    int value = Integer.parseInt(event.substring(9));
                    assert value == ++previous : event;
                    fixes++;
                } else if (event.startsWith("failed\t")) {
                    assert event.startsWith("failed\t800 positions");
                    notices++;
                } else {
                    assert (completions == 0 && event.startsWith("picked\t"))
                            || (completions == 1 && event.equals("ended\tdxf:7"));
                    completions++;
                }
            }
        }
        assert fixes == 1200 && notices == 1 && completions == 2;
        assert events.take().isEmpty();
        LocationProviders providers = new LocationProviders();
        providers.add("network");
        // Approximate permission registered only network: enabled but unrequested GPS cannot
        // keep a dead network listener looking alive.
        assert !providers.anyEnabled(provider -> provider.equals("gps"));
        assert providers.anyEnabled(provider -> provider.equals("network"));
        providers.add("gps");
        assert providers.anyEnabled(provider -> provider.equals("gps"));
        providers.clear();
        assert !providers.anyEnabled(provider -> true);
        overwriteCutsALongerDocument();
        System.out.println("Phone contract checks passed");
    }

    /** A project written over a longer one leaves none of the old bytes after it. */
    private static void overwriteCutsALongerDocument() {
        try {
            java.io.File file = java.io.File.createTempFile("document", ".rgis");
            try {
                java.nio.file.Files.write(file.toPath(), "an older and longer project".getBytes("UTF-8"));
                try (java.io.RandomAccessFile open = new java.io.RandomAccessFile(file, "rw")) {
                    DocumentWrite.overwrite(open.getChannel(), "new".getBytes("UTF-8"));
                }
                String held = new String(java.nio.file.Files.readAllBytes(file.toPath()), "UTF-8");
                assert held.equals("new") : held;
                // Longer than before: all of it.
                try (java.io.RandomAccessFile open = new java.io.RandomAccessFile(file, "rw")) {
                    DocumentWrite.overwrite(open.getChannel(), "a newer and longer project, again".getBytes("UTF-8"));
                }
                held = new String(java.nio.file.Files.readAllBytes(file.toPath()), "UTF-8");
                assert held.equals("a newer and longer project, again") : held;
            } finally {
                file.delete();
            }
        } catch (java.io.IOException e) {
            throw new AssertionError(e);
        }
    }
}
