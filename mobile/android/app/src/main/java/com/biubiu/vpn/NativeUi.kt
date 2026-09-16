package com.biubiu.vpn

import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.view.Gravity
import android.widget.*

/** Small native view vocabulary shared by the three screens and their sheets. */
internal class NativeUi(private val context: Context) {
    val blue = Color.rgb(37, 99, 235)
    val ink = Color.rgb(17, 24, 39)
    val muted = Color.rgb(100, 116, 139)
    val background = Color.rgb(248, 250, 252)
    val soft = Color.rgb(239, 246, 255)
    fun dp(value: Int) = (value * context.resources.displayMetrics.density).toInt()
    fun shape(color: Int, radius: Int = 16, border: Boolean = false) = GradientDrawable().apply {
        setColor(color); cornerRadius = dp(radius).toFloat()
        if (border) setStroke(dp(1), Color.rgb(223, 230, 239))
    }
    fun column(padding: Int = 0) = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL; setPadding(dp(padding), dp(padding), dp(padding), dp(padding))
    }
    fun text(parent: LinearLayout, value: String, size: Float = 14f, color: Int = ink, bold: Boolean = false) = TextView(context).apply {
        text = value; textSize = size; setTextColor(color); setLineSpacing(dp(3).toFloat(), 1f)
        if (bold) typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        setPadding(0, dp(6), 0, dp(6)); parent.addView(this, LinearLayout.LayoutParams(-1, -2))
    }
    fun gap(parent: LinearLayout, height: Int = 12) { parent.addView(android.view.View(context), LinearLayout.LayoutParams(1, dp(height))) }
    fun card(parent: LinearLayout, tint: Int = Color.WHITE) = column(16).apply {
        background = shape(tint, 16, true)
        parent.addView(this, LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(12) })
    }
    fun button(parent: LinearLayout, title: String, primary: Boolean = false, action: () -> Unit) = Button(context).apply {
        text = title
        val menuIcon = mapOf("服务地址" to 5, "应用更新" to 6, "修改密码" to 7, "诊断与日志" to 1, "系统 VPN 设置" to 4, "关于易链" to 8)[title]
        if (menuIcon != null) {
            gravity = Gravity.CENTER_VERTICAL or Gravity.START
            setCompoundDrawablesWithIntrinsicBounds(NativeLineIcon(dp(20), muted, menuIcon), null, NativeLineIcon(dp(18), muted, 3), null)
            compoundDrawablePadding = dp(12)
        }
        textSize = 14f; isAllCaps = false; minHeight = dp(50)
        setPadding(dp(16), dp(10), dp(16), dp(10)); setTextColor(if (primary) Color.WHITE else if (menuIcon != null) ink else blue)
        background = shape(if (primary) blue else Color.WHITE, 13, !primary)
        setOnClickListener { action() }
        parent.addView(this, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(8); bottomMargin = dp(4) })
    }
    fun field(parent: LinearLayout, title: String, type: Int) = EditText(context).apply {
        hint = title; inputType = type; setSingleLine(); textSize = 15f
        setTextColor(ink); setHintTextColor(muted); minHeight = dp(52)
        setPadding(dp(14), dp(12), dp(14), dp(12)); background = shape(Color.WHITE, 12, true)
        parent.addView(this, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(8); bottomMargin = dp(8) })
    }
    fun centered(view: TextView) { view.gravity = Gravity.CENTER }
}

/** Scale-independent line glyphs; no platform-dependent emoji rendering. */
internal class NativeLineIcon(private val size: Int, color: Int, private val kind: Int) : android.graphics.drawable.Drawable() {
    private val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply { this.color = color; style = android.graphics.Paint.Style.STROKE; strokeWidth = 1.7f; strokeCap = android.graphics.Paint.Cap.ROUND; strokeJoin = android.graphics.Paint.Join.ROUND }
    override fun getIntrinsicWidth() = size
    override fun getIntrinsicHeight() = size
    override fun draw(canvas: android.graphics.Canvas) {
        canvas.save(); canvas.translate(bounds.left.toFloat(), bounds.top.toFloat()); canvas.scale(bounds.width() / 24f, bounds.height() / 24f)
        when (kind) {
            0 -> { canvas.drawLine(12f, 3f, 12f, 11f, paint); canvas.drawArc(4f, 4f, 20f, 20f, -48f, 276f, false, paint) }
            1 -> { val path = android.graphics.Path().apply { moveTo(2f, 12f); lineTo(7f, 12f); lineTo(10f, 4f); lineTo(14f, 20f); lineTo(17f, 12f); lineTo(22f, 12f) }; canvas.drawPath(path, paint) }
            4 -> { val path = android.graphics.Path().apply { moveTo(12f, 3f); lineTo(4f, 6f); lineTo(4f, 12f); quadTo(4f, 17f, 12f, 21f); quadTo(20f, 17f, 20f, 12f); lineTo(20f, 6f); close(); moveTo(8f, 12f); lineTo(11f, 15f); lineTo(16f, 9f) }; canvas.drawPath(path, paint) }
            5 -> { canvas.drawRoundRect(3f, 3f, 21f, 10f, 2f, 2f, paint); canvas.drawRoundRect(3f, 14f, 21f, 21f, 2f, 2f, paint); canvas.drawPoint(7f, 6.5f, paint); canvas.drawPoint(7f, 17.5f, paint) }
            6 -> { canvas.drawLine(12f, 3f, 12f, 15f, paint); canvas.drawLine(7f, 10f, 12f, 15f, paint); canvas.drawLine(12f, 15f, 17f, 10f, paint); canvas.drawLine(4f, 20f, 20f, 20f, paint) }
            7 -> { canvas.drawCircle(15f, 8f, 5f, paint); canvas.drawLine(11.5f, 11.5f, 3f, 20f, paint); canvas.drawLine(5f, 18f, 8f, 21f, paint) }
            8 -> { canvas.drawCircle(12f, 12f, 9f, paint); canvas.drawPoint(12f, 7f, paint); canvas.drawLine(12f, 11f, 12f, 17f, paint) }
            3 -> { canvas.drawLine(9f, 6f, 15f, 12f, paint); canvas.drawLine(15f, 12f, 9f, 18f, paint) }
            else -> { canvas.drawCircle(12f, 7f, 3.5f, paint); canvas.drawArc(4f, 13f, 20f, 25f, 180f, 180f, false, paint) }
        }
        canvas.restore()
    }
    override fun setAlpha(alpha: Int) { paint.alpha = alpha }
    override fun setColorFilter(colorFilter: android.graphics.ColorFilter?) { paint.colorFilter = colorFilter }
    @Deprecated("Drawable opacity") override fun getOpacity() = android.graphics.PixelFormat.TRANSLUCENT
}
