package com.nunyavpn.app

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * The app's one activity.
 *
 * Android 15 and later draw every app edge to edge, under the status bar and the navigation bar,
 * and leave keeping clear of them to the app. The page could do it with `env(safe-area-inset-*)`,
 * but not every WebView a phone carries reports those insets, and one that does not put the list
 * title under the clock. So the webview is padded here, by the system bars, a display cutout and
 * the keyboard together: the page lays out in exactly the space it can use, and a field being
 * typed into is never under the keyboard. The strips behind the bars are the theme's background.
 */
class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val clear = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or
          WindowInsetsCompat.Type.displayCutout() or
          WindowInsetsCompat.Type.ime(),
      )
      view.setPadding(clear.left, clear.top, clear.right, clear.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }
}
