package com.aieink.pokedashboard;

import android.graphics.Color;

/**
 * Shared 16-level grayscale palette and geometry tokens tuned for e-ink panels.
 */
final class EinkTheme {
    static final int PAPER = Color.rgb(247, 247, 244);
    static final int PAPER_ALT = Color.rgb(239, 236, 228);
    static final int CHIP_BG = Color.rgb(234, 232, 225);
    static final int INK = Color.rgb(17, 17, 17);
    static final int MUTED = Color.rgb(85, 84, 80);
    static final int DISABLED = Color.rgb(136, 134, 128);
    static final int HAIR = Color.rgb(198, 196, 188);
    static final int TRACK = Color.rgb(227, 225, 218);
    static final int TRACK_DARK = Color.rgb(218, 215, 206);
    static final int WHITE = Color.WHITE;

    static final float DESIGN_WIDTH = 758f;
    static final float DESIGN_HEIGHT = 1024f;
    static final float CARD_RADIUS = 12f;
    static final float CARD_STROKE = 1.6f;
    static final float ALERT_STROKE = 3f;

    private EinkTheme() { }
}
