package org.tvcontrol.fixture;

import android.app.Activity;
import android.os.Bundle;
import android.text.InputType;
import android.view.KeyEvent;
import android.widget.EditText;
import android.widget.LinearLayout;

/** No permissions, network, media, services or storage. Owned disposable AVD only. */
public final class FixtureActivity extends Activity {
    private EditText editor;
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        String mode = getIntent().getStringExtra("mode");
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        editor = new EditText(this);
        editor.setId(R.id.editor);
        editor.setSingleLine(true);
        editor.setInputType(InputType.TYPE_CLASS_TEXT);
        editor.setShowSoftInputOnFocus(false);
        EditText other = new EditText(this);
        other.setId(R.id.other);
        other.setShowSoftInputOnFocus(false);
        layout.addView(editor, new LinearLayout.LayoutParams(-1, 160));
        layout.addView(other, new LinearLayout.LayoutParams(-1, 160));
        setContentView(layout);
        configureEditors(mode, editor, other);
    }
    private static void configureEditors(String mode, EditText editor, EditText other) {
        if ("hint".equals(mode)) editor.setHint("fixture hint");
        if ("password".equals(mode)) editor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        if ("visible-password".equals(mode)) editor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD);
        boolean seeded = !"hint".equals(mode) && !"key-remap".equals(mode) && !"empty-null".equals(mode);
        if (seeded) editor.setText("seed");
        if ("focus".equals(mode)) { other.setText("other"); other.setSelection(other.getText().length()); }
        editor.requestFocus();
        if ("selection".equals(mode)) editor.setSelection(1);
        else if (seeded) editor.setSelection(editor.getText().length());
    }
    @Override public boolean dispatchKeyEvent(KeyEvent event) {
        // Deliberate fixture-only layout mismatch, independent of installed IME.
        if (event.getKeyCode() == KeyEvent.KEYCODE_A && editor.hasFocus()) {
            if (event.getAction() == KeyEvent.ACTION_DOWN) editor.append(event.isShiftPressed() ? "Q" : "q");
            return true;
        }
        return super.dispatchKeyEvent(event);
    }
}
