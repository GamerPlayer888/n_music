package com.enn3developer.n_music

import android.Manifest.permission.POST_NOTIFICATIONS
import android.Manifest.permission.READ_MEDIA_AUDIO
import android.annotation.SuppressLint
import android.app.NativeActivity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import androidx.annotation.OptIn
import androidx.annotation.RequiresApi
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.R.drawable
import androidx.annotation.Keep
import androidx.core.net.toUri
import android.graphics.Color
import android.content.res.Configuration
import androidx.core.view.WindowCompat


@OptIn(UnstableApi::class)
class MainActivity : NativeActivity() {
    companion object {
        init {
            // Load the STL first to workaround issues on old Android versions:
            // "if your app targets a version of Android earlier than Android 4.3
            // (Android API level 18),
            // and you use libc++_shared.so, you must load the shared library before any other
            // library that depends on it."
            // See https://developer.android.com/ndk/guides/cpp-support#shared_runtimes
            //System.loadLibrary("c++_shared");

            // Load the native library.
            // The name "android-game" depends on your CMake configuration, must be
            // consistent here and inside AndroidManifest.xml
            System.loadLibrary("n_player")
        }

        const val NOTIFICATION_NAME_SERVICE = "NPlayer"
        const val NOTIFICATION_ID = 1
        const val CHANNEL_ID = "NMusic"
        const val ASK_DIRECTORY = 0
        const val ASK_FILE = 1
        const val REQUEST_PERMISSION_CODE = 1
        const val CUSTOM_REPLAY_ON = "com.enn3developer.action.TOGGLE_REPEAT_ALL"
        const val CUSTOM_REPLAY_OFF = "com.enn3developer.action.TOGGLE_REPEAT_NONE"
        const val ACTIONS = PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_SKIP_TO_NEXT or PlaybackState.ACTION_SKIP_TO_PREVIOUS or PlaybackState.ACTION_SEEK_TO
    }

    private var theme: Int = 0 // App theme: 0 = System, 1 = Light, 2 = Dark

    @SuppressLint("RestrictedApi")
    // It's the playback in the notification
    public var playback: PlaybackState.Builder? = null

    // It's used to set metadata of the song and playback
    public var mediaSession: MediaSession? = null

    // We set here mediaSession token for style
    private var notification: Notification.Builder? = null

    // Called when app is open first time
    @Keep
    private external fun start(activity: MainActivity)

    @Keep
    private external fun gotDirectory(directory: String)

    @Keep
    private external fun gotFile(file: String)

	@Keep
	private external fun onVisibilityChanged(isVisible: Boolean)

    private val bluetoothBroadcastReceiver = object : BroadcastReceiver() {
        override fun onReceive(p0: Context?, p1: Intent?) {
        	if(intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY
                && mediaSession?.controller?.playbackState?.state == PlaybackState.STATE_PLAYING) {
                mediaSession?.controller?.transportControls?.pause()
        	}
        }
    }

    private fun askDirectoryWithPermission() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
        }
        startActivityForResult(intent, ASK_DIRECTORY)
    }

	@Suppress("unused")
	@Keep
    private fun set_theme(value: Int) {
		theme = value
		updateStatusBarAppearance()
	}

    @Suppress("unused")
    @Keep
    private fun askDirectory() {
        println("asking directory")
        //Check if permission has been granted
        if (!checkPermissions()) {
            requestPermissions()
        } else {
            askDirectoryWithPermission()
        }
    }

    @Suppress("unused")
    @Keep
    private fun askFile() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
        }
        startActivityForResult(intent, ASK_FILE)
    }

    @Suppress("unused")
    @Keep
    private fun set_clipboard_text(text: String){
        val clipboard: ClipboardManager = getSystemService(CLIPBOARD_SERVICE) as ClipboardManager
        val clip = ClipData.newPlainText(text, text)
        clipboard.setPrimaryClip(clip)
    }

    @Suppress("unused")
    @Keep
    private fun openLink(link: String) {
        val browserIntent = Intent(Intent.ACTION_VIEW, link.toUri())
        startActivity(browserIntent)
    }

    private fun updateStatusBarAppearance() {
            runOnUiThread {
                val window = this.window
                window.addFlags(android.view.WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS)

                val isLight = when (theme) {
                    1 -> true
                    2 -> false
                    else -> {
                        val currentNightMode = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK
                        currentNightMode == Configuration.UI_MODE_NIGHT_NO
                    }
                }

                window.statusBarColor = if (isLight) Color.WHITE else Color.BLACK

                WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightStatusBars = isLight
            }
        }

    override fun onConfigurationChanged(newConfig: Configuration) {
		super.onConfigurationChanged(newConfig)
		updateStatusBarAppearance()
	}

    override fun onStart() {
        super.onStart()
        onVisibilityChanged(true)
    }

    override fun onStop() {
        super.onStop()
        onVisibilityChanged(false)
    }

    @SuppressLint("RestrictedApi")
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    @Suppress("unused")
    @Keep
    private fun createNotification() {
        if (!checkPermissions()) {
            requestPermissions()
        }
        val TAG = "PlaybackService"
        mediaSession = MediaSession(applicationContext, TAG).apply {
            isActive = true;
        }
        val handler = Handler(Looper.getMainLooper())
        handler.post {
            mediaSession?.setCallback(MediaCallback(mediaSession!!, this))
        }
        val bluetoothReceiver = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        applicationContext.registerReceiver(bluetoothBroadcastReceiver, bluetoothReceiver)
        playback = PlaybackState.Builder()
            .addCustomAction(PlaybackState.CustomAction.Builder(CUSTOM_REPLAY_OFF,
                "REPEAT OFF", drawable.media3_icon_repeat_off).build())
            .setActions(ACTIONS)
            .setActiveQueueItemId(ACTIONS)
        val channel = NotificationChannel(
            CHANNEL_ID,
            NOTIFICATION_NAME_SERVICE,
            NotificationManager.IMPORTANCE_LOW
        )
        playback?.setState(
            PlaybackState.STATE_PLAYING,
            0L, 1.0f
        )
        mediaSession?.setPlaybackState(playback?.build())
        NotificationManagerCompat.from(applicationContext).createNotificationChannel(channel)
        notification = Notification.Builder(applicationContext, CHANNEL_ID).apply {
            setSmallIcon(R.drawable.ic_launcher_monochrome)
            style = Notification.MediaStyle().setMediaSession(mediaSession?.sessionToken)
            setOngoing(true)
        }
        val builtNotification = notification?.build()
        PlaybackService.currentNotification = builtNotification
        val serviceIntent = Intent(applicationContext, PlaybackService::class.java)
        applicationContext.startForegroundService(serviceIntent)
    }

    @Keep
    private fun changeLoopingStatus(status: Boolean) {
        val pos = mediaSession?.controller?.playbackState?.position ?: 0L
        val state = mediaSession?.controller?.playbackState?.state ?: PlaybackState.STATE_NONE

        this.playback = PlaybackState.Builder()
            .setActions(ACTIONS)

        if (status) {
            this.playback?.addCustomAction(
                PlaybackState.CustomAction.Builder(
                    CUSTOM_REPLAY_ON,
                    "REPEAT ON",
                    drawable.media3_icon_repeat_all
                ).build()
            )
        } else {
            this.playback?.addCustomAction(
                PlaybackState.CustomAction.Builder(
                    CUSTOM_REPLAY_OFF,
                    "REPEAT OFF",
                    drawable.media3_icon_repeat_off
                ).build()
            )
        }

        this.playback?.setState(state, pos, 1.0f)

        mediaSession?.setPlaybackState(this.playback?.build())

        val notificationManager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        notification?.let {
            notificationManager.notify(NOTIFICATION_ID, it.build())
        }
    }

    @Keep
    private fun changePlaybackStatus(status: Boolean) {
        val playbackState = mediaSession?.controller?.playbackState
        playbackState?.position?.let {
            playback?.setState(
                if (status)
                    PlaybackState.STATE_PLAYING
                else PlaybackState.STATE_PAUSED,
                it, 1.0f
            )
        }
        mediaSession?.setPlaybackState(playback?.build())
    }

    @Keep
    private fun changePlaybackSeek(pos: Double) {
        mediaSession?.controller?.playbackState?.state?.let {
            playback?.setState(
                it,
                pos.toLong() * 1000,
                1.0f
            )
        }
        mediaSession?.setPlaybackState(playback?.build())
    }

    @OptIn(UnstableApi::class)
    @SuppressLint("RestrictedApi")
    @Suppress("unused")
    @Keep
    private fun changeNotification(
        title: String,
        artists: String,
        coverPath: String,
        songLength: Double
    ) {
        var intent = applicationContext.packageManager.getLaunchIntentForPackage(packageName)

        if (intent == null) {
            intent = Intent(applicationContext, MainActivity::class.java)
        }

        intent.flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED

        val pendingIntent =
            PendingIntent.getActivity(
                applicationContext, 0, intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            )

        val duration = songLength.toLong() * 1000
        val metadata = MediaMetadata.Builder()
            .apply {
                putString(MediaMetadata.METADATA_KEY_TITLE, title)
                putString(MediaMetadata.METADATA_KEY_ARTIST, artists)
                putLong(MediaMetadata.METADATA_KEY_DURATION, duration)
                if (coverPath.isNotEmpty()) {
                    val cover = BitmapFactory.decodeFile(coverPath)
                    putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, cover)
                }
            }
            .build()
        mediaSession?.controller?.playbackState?.state?.let { playback?.setState(it, 0L, 1.0f) }
        mediaSession?.apply {
            setMetadata(metadata)
            setPlaybackState(playback?.build())
        }
        notification?.apply {
            setContentTitle(title)
            setContentText(artists)
            setContentIntent(pendingIntent)
            val cover = BitmapFactory.decodeFile(coverPath)
            if (cover != null) {
                setLargeIcon(cover)
            }
        }

        val builtNotification = notification?.build()
        PlaybackService.currentNotification = builtNotification

        with(getSystemService(NOTIFICATION_SERVICE) as NotificationManager) {
            if (ActivityCompat.checkSelfPermission(
                    applicationContext,
                    POST_NOTIFICATIONS
                ) != PackageManager.PERMISSION_GRANTED
            ) {
                return@with
            }
            builtNotification?.let {
                notify(NOTIFICATION_ID, it)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        start(this)
    }

    override fun onDestroy() {
        val notificationManager =
            getSystemService(NOTIFICATION_SERVICE) as NotificationManager
        notificationManager.cancel(NOTIFICATION_ID)

        val serviceIntent = Intent(applicationContext, PlaybackService::class.java)
        applicationContext.stopService(serviceIntent)

        super.onDestroy()
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (resultCode == RESULT_OK) {
            println("activity result ok")
            if (requestCode == ASK_DIRECTORY) {
                println("activity ask directory")
                data?.data?.also { uri ->
                    println("got data")
                    if (uri.path != null) {
                        println("path is not null")
                        val contentResolver = applicationContext.contentResolver
                        val takeFlags: Int = Intent.FLAG_GRANT_READ_URI_PERMISSION
                        contentResolver.takePersistableUriPermission(uri, takeFlags)
                        val path = uri.path!!.replace("/tree/primary:", "/storage/emulated/0/")
                        Toast.makeText(applicationContext, "Loading music...", Toast.LENGTH_LONG)
                            .show()
                        gotDirectory(path)
                    }
                }
            } else if (requestCode == ASK_FILE) {
                data?.data?.also { uri ->
                    val path = uri.path!!.replace("/tree/primary:", "/storage/emulated/0/")
                    gotFile(path)
                }
            }
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)

        if (requestCode == REQUEST_PERMISSION_CODE) {
            if (grantResults.isEmpty()) return;

            if (grantResults[0] == PackageManager.PERMISSION_GRANTED) {
                 Toast.makeText(applicationContext, "Permission granted", Toast.LENGTH_SHORT)
                     .show()
                 askDirectoryWithPermission()
			} else {
                Toast.makeText(applicationContext, "Oops, relaunch app please", Toast.LENGTH_LONG)
                    .show()
            }
        }
    }

    @SuppressLint("InlinedApi")
    fun checkPermissions(): Boolean {
        val readMediaAudio = ContextCompat.checkSelfPermission(applicationContext, READ_MEDIA_AUDIO)
        val grantNotification =
            ContextCompat.checkSelfPermission(applicationContext, POST_NOTIFICATIONS)
        return (readMediaAudio == PackageManager.PERMISSION_GRANTED) && (grantNotification == PackageManager.PERMISSION_GRANTED)
    }

    @SuppressLint("InlinedApi")
    private fun requestPermissions() {
        ActivityCompat.requestPermissions(
            this,
            arrayOf(READ_MEDIA_AUDIO, POST_NOTIFICATIONS),
            REQUEST_PERMISSION_CODE
        )
    }
}