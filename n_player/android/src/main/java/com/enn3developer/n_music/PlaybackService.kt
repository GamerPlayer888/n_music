package com.enn3developer.n_music

import android.app.Notification
import android.app.Service
import android.content.Intent
import android.os.IBinder

class PlaybackService : Service() {

    companion object {
        var currentNotification: Notification? = null
    }

    override fun onBind(p0: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {

        currentNotification?.let {
            startForeground(MainActivity.NOTIFICATION_ID, it)
        }

        return START_STICKY
    }
}