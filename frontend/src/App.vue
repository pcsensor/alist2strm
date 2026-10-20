<template>
  <div class="app-layout">
    <!-- 顶部导航 -->
    <header class="header">
      <div class="header-inner">
        <div class="logo">
          <span class="logo-icon">🎬</span>
          <div>
            <h1 class="logo-title">AList2STRM</h1>
            <p class="logo-sub">百度网盘 302 直链 STRM 生成与管理面板</p>
          </div>
        </div>
        <div class="header-actions">
          <button class="btn btn-secondary" @click="showAlistModal = true">
            <span>⚙️</span> AList 连接配置
          </button>
          <button class="btn btn-primary" @click="openCreateTaskModal">
            <span>➕</span> 新建 STRM 任务
          </button>
        </div>
      </div>
    </header>

    <main class="main-content">
      <!-- 状态指示条 -->
      <div class="status-banner" :class="{ 'connected': alistConnected }">
        <div class="status-left">
          <span class="status-dot"></span>
          <span>AList 状态: <strong>{{ alistConnected ? '已连接 (' + alistUser + ')' : '未测试或未连接' }}</strong></span>
          <span class="status-sep">|</span>
          <span class="status-url">{{ appConfig.alist?.base_url }}</span>
        </div>
        <button class="btn-text" @click="testCurrentAlist">快速连通性测试</button>
      </div>

      <!-- 任务列表区域 -->
      <section class="tasks-section">
        <div class="section-header">
          <h2>STRM 同步任务 ({{ tasks.length }})</h2>
          <span class="text-muted">支持定时 Cron 自动运行，或随时点击「立即执行」手动触发</span>
        </div>

        <div v-if="tasks.length === 0" class="empty-state">
          <div class="empty-icon">📂</div>
          <p>暂无任务，点击右上角「新建 STRM 任务」开始</p>
        </div>

        <div v-else class="task-grid">
          <div v-for="task in tasks" :key="task.id" class="task-card">
            <div class="task-card-header">
              <div class="task-title-wrap">
                <h3 class="task-title">{{ task.name }}</h3>
                <span :class="['badge', 'badge-' + (task.runtime?.status || 'idle')]">
                  {{ statusText(task.runtime?.status) }}
                </span>
              </div>
              <div class="task-mode-tag">
                {{ task.mode === 'AlistURL' ? '🔗 302 AlistURL (百度网盘推荐)' : '⚡ RawURL 模式' }}
              </div>
            </div>

            <div class="task-card-body">
              <div class="info-row">
                <span class="info-label">云盘目录:</span>
                <span class="info-val code-text" :title="task.source_dir">{{ task.source_dir }}</span>
              </div>
              <div class="info-row">
                <span class="info-label">本地目录:</span>
                <span class="info-val code-text" :title="task.target_dir">{{ task.target_dir }}</span>
              </div>
              <div class="info-row">
                <span class="info-label">定时规则:</span>
                <span class="info-val">{{ task.cron || '未配置 (纯手动触发)' }}</span>
              </div>
              <div class="info-row">
                <span class="info-label">并发 / 增量:</span>
                <span class="info-val">{{ task.concurrency }} 线程 / {{ task.overwrite ? '每次覆盖' : '智能增量跳过' }}</span>
              </div>

              <!-- 上次执行摘要 -->
              <div class="run-summary" v-if="task.runtime?.last_run">
                <div class="summary-title">上次运行: {{ formatDate(task.runtime.last_run) }} (耗时 {{ task.runtime.last_duration_secs?.toFixed(1) }}s)</div>
                <div v-if="task.runtime.last_stats" class="summary-stats">
                  <span class="stat-tag">扫描目录: {{ task.runtime.last_stats.scanned_dirs }}</span>
                  <span class="stat-tag success">生成: {{ task.runtime.last_stats.created_strm }}</span>
                  <span class="stat-tag">跳过: {{ task.runtime.last_stats.skipped_strm }}</span>
                  <span v-if="task.runtime.last_stats.deleted_strm > 0" class="stat-tag danger">删除: {{ task.runtime.last_stats.deleted_strm }}</span>
                </div>
                <div v-if="task.runtime.last_error" class="summary-error">
                  ❌ {{ task.runtime.last_error }}
                </div>
              </div>
            </div>

            <!-- 卡片操作按钮 -->
            <div class="task-card-footer">
              <button 
                class="btn btn-success" 
                :disabled="task.runtime?.status === 'running'"
                @click="triggerTask(task.id)"
              >
                <span v-if="task.runtime?.status === 'running'">⏳ 运行中...</span>
                <span v-else>▶ 立即运行</span>
              </button>
              
              <div class="footer-sub-actions">
                <button class="btn btn-secondary" @click="openLogs(task.id)">
                  📜 查看日志
                </button>
                <button class="btn btn-secondary" @click="editTask(task)">
                  ✏️ 编辑
                </button>
                <button class="btn btn-danger" @click="deleteTask(task.id)">
                  🗑️
                </button>
              </div>
            </div>
          </div>
        </div>
      </section>

      <!-- 实时日志终端展示板 -->
      <section class="logs-section">
        <div class="section-header">
          <div class="log-header-left">
            <h2>实时同步日志终端</h2>
            <span class="live-dot"></span>
            <span class="text-muted">SSE 实时流传输</span>
          </div>
          <div class="log-actions">
            <button class="btn btn-secondary btn-sm" @click="logs = []">清空日志</button>
          </div>
        </div>
        <div class="terminal" ref="terminalBox">
          <div v-if="logs.length === 0" class="terminal-empty">
            等待任务触发... 点击上方「立即运行」即可在此查看实时扫描进度。
          </div>
          <div v-for="(item, idx) in logs" :key="idx" class="terminal-line">
            <span class="terminal-time">[{{ item.time }}]</span>
            <span class="terminal-msg">{{ item.message }}</span>
          </div>
        </div>
      </section>
    </main>

    <!-- AList 配置弹窗 -->
    <div v-if="showAlistModal" class="modal-overlay" @click.self="showAlistModal = false">
      <div class="modal-content">
        <div class="modal-header">
          <h3>AList 凭证与连接配置</h3>
          <button class="modal-close" @click="showAlistModal = false">✕</button>
        </div>
        <div class="modal-body">
          <div class="form-group">
            <label>AList 服务地址 (Base URL):</label>
            <input v-model="editAlist.base_url" placeholder="如 http://127.0.0.1:5244 或 http://192.168.1.100:5244" />
            <small>用于本项目与 AList 服务端通信进行目录扫描</small>
          </div>
          <div class="form-group">
            <label>公网/外部播放地址 (Public URL，选填):</label>
            <input v-model="editAlist.public_url" placeholder="如 https://alist.mydomain.com:5244" />
            <small>供播放器远程访问。若填入，生成的 STRM 内将使用此域名替换内部 Base URL</small>
          </div>
          <div class="form-group">
            <label>AList 令牌 (Token):</label>
            <input v-model="editAlist.token" type="password" placeholder="AList 后台「设置 - 其他」中的 Token" />
            <small>使用 Token 认证，无需账号密码</small>
          </div>
          <div class="form-group">
            <label>请求间隔延迟 (秒):</label>
            <input v-model.number="editAlist.wait_time" type="number" step="0.01" min="0" />
            <small>每次调用 AList 接口后的休眠等待时间，防止请求过于频繁被百度网盘限速</small>
          </div>

          <div v-if="testResult" class="test-result-box" :class="{ 'success': testResult.success }">
            {{ testResult.message }}
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn btn-secondary" :disabled="testing" @click="testAlistConfig">
            {{ testing ? '测试中...' : '测试连通性' }}
          </button>
          <button class="btn btn-primary" @click="saveAlistConfig">保存配置</button>
        </div>
      </div>
    </div>

    <!-- 任务新建/编辑弹窗 -->
    <div v-if="showTaskModal" class="modal-overlay" @click.self="showTaskModal = false">
      <div class="modal-content">
        <div class="modal-header">
          <h3>{{ editingTaskId ? '编辑 STRM 任务' : '新建 STRM 任务' }}</h3>
          <button class="modal-close" @click="showTaskModal = false">✕</button>
        </div>
        <div class="modal-body">
          <div class="form-group">
            <label>任务名称:</label>
            <input v-model="taskForm.name" placeholder="如：百度网盘4K电影" />
          </div>
          <div class="form-group">
            <label>AList 影视挂载源目录:</label>
            <input v-model="taskForm.source_dir" placeholder="如：/百度网盘/影视 或 /Movies" />
            <small>AList 中的虚拟路径，支持递归遍历所有子目录</small>
          </div>
          <div class="form-group">
            <label>本地 STRM 输出目录:</label>
            <input v-model="taskForm.target_dir" placeholder="如：/media/movies 或 ./strm/movies" />
            <small>存放生成的 .strm 文件的本地目录，保持完整子目录层级供 Emby/Jellyfin 识别</small>
          </div>
          <div class="form-row">
            <div class="form-group flex-1">
              <label>STRM 链接生成模式:</label>
              <select v-model="taskForm.mode">
                <option value="AlistURL">AlistURL (推荐，最适合百度网盘302)</option>
                <option value="RawURL">RawURL (直接获取上游真实直链)</option>
              </select>
            </div>
            <div class="form-group flex-1">
              <label>扫描并发数:</label>
              <input v-model.number="taskForm.concurrency" type="number" min="1" max="50" />
            </div>
          </div>
          <div class="form-group">
            <label>定时运行 Cron 表达式 (选填):</label>
            <input v-model="taskForm.cron" placeholder="如 0 0 3 * * * (每天凌晨3点)；留空表示仅手动触发" />
          </div>
          <div class="form-check-group">
            <label class="check-item">
              <input type="checkbox" v-model="taskForm.overwrite" />
              <span>覆盖已有同名文件 (不勾选则仅增量生成，本地已存在的秒级跳过)</span>
            </label>
            <label class="check-item">
              <input type="checkbox" v-model="taskForm.sync_delete" />
              <span>同步删除 (云端已删掉的影视，同步清理本地残留的 .strm)</span>
            </label>
            <label class="check-item">
              <input type="checkbox" v-model="taskForm.flatten" />
              <span>平铺模式 (不保留多层子文件夹，所有 .strm 生成到同一级)</span>
            </label>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn btn-secondary" @click="showTaskModal = false">取消</button>
          <button class="btn btn-primary" @click="saveTask">保存任务</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, onMounted, nextTick } from 'vue'

const appConfig = ref({ alist: {}, tasks: [] })
const tasks = ref([])
const logs = ref([])
const terminalBox = ref(null)

const alistConnected = ref(false)
const alistUser = ref('')
const testing = ref(false)
const testResult = ref(null)

const showAlistModal = ref(false)
const editAlist = ref({ base_url: '', public_url: '', token: '', wait_time: 0.05 })

const showTaskModal = ref(false)
const editingTaskId = ref(null)
const taskForm = ref({
  name: '',
  source_dir: '',
  target_dir: '',
  mode: 'AlistURL',
  cron: '',
  concurrency: 10,
  overwrite: false,
  sync_delete: false,
  flatten: false,
  video_extensions: ['.mp4', '.mkv', '.avi', '.ts', '.iso', '.mov', '.wmv', '.m2ts', '.rmvb']
})

// 加载配置
async function loadConfig() {
  try {
    const res = await fetch('/api/config')
    const json = await res.json()
    if (json.success) {
      appConfig.value = json.data
      editAlist.value = { ...json.data.alist }
    }
  } catch (e) {
    console.error('加载配置失败:', e)
  }
}

// 加载任务列表
async function loadTasks() {
  try {
    const res = await fetch('/api/tasks')
    const json = await res.json()
    if (json.success) {
      tasks.value = json.data
    }
  } catch (e) {
    console.error('加载任务失败:', e)
  }
}

// 测试当前配置的 AList
async function testCurrentAlist() {
  if (!appConfig.value.alist?.base_url) return
  testing.value = true
  testResult.value = null
  try {
    const res = await fetch('/api/alist/test', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(appConfig.value.alist)
    })
    const json = await res.json()
    if (json.success && json.data.connected) {
      alistConnected.value = true
      alistUser.value = json.data.username
      testResult.value = { success: true, message: `✅ 连接成功！当前用户: ${json.data.username}，根路径: ${json.data.base_path}` }
    } else {
      alistConnected.value = false
      testResult.value = { success: false, message: `❌ 连接失败: ${json.error}` }
    }
  } catch (e) {
    alistConnected.value = false
    testResult.value = { success: false, message: `❌ 网络错误: ${e.message}` }
  } finally {
    testing.value = false
  }
}

// 在弹窗里测试
async function testAlistConfig() {
  testing.value = true
  testResult.value = null
  try {
    const res = await fetch('/api/alist/test', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(editAlist.value)
    })
    const json = await res.json()
    if (json.success && json.data.connected) {
      testResult.value = { success: true, message: `✅ 连接成功！用户: ${json.data.username}，根路径: ${json.data.base_path}` }
    } else {
      testResult.value = { success: false, message: `❌ 失败: ${json.error}` }
    }
  } catch (e) {
    testResult.value = { success: false, message: `❌ 错误: ${e.message}` }
  } finally {
    testing.value = false
  }
}

// 保存 AList 配置
async function saveAlistConfig() {
  try {
    const res = await fetch('/api/config', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ alist: editAlist.value })
    })
    const json = await res.json()
    if (json.success) {
      showAlistModal.value = false
      await loadConfig()
      testCurrentAlist()
    } else {
      alert('保存失败: ' + json.error)
    }
  } catch (e) {
    alert('请求失败: ' + e.message)
  }
}

// 打开创建任务弹窗
function openCreateTaskModal() {
  editingTaskId.value = null
  taskForm.value = {
    name: '百度网盘电影',
    source_dir: '/百度网盘/电影',
    target_dir: './strm/movies',
    mode: 'AlistURL',
    cron: '',
    concurrency: 10,
    overwrite: false,
    sync_delete: false,
    flatten: false,
    video_extensions: ['.mp4', '.mkv', '.avi', '.ts', '.iso', '.mov', '.wmv', '.m2ts', '.rmvb']
  }
  showTaskModal.value = true
}

// 编辑任务
function editTask(t) {
  editingTaskId.value = t.id
  taskForm.value = {
    name: t.name,
    source_dir: t.source_dir,
    target_dir: t.target_dir,
    mode: t.mode,
    cron: t.cron || '',
    concurrency: t.concurrency || 10,
    overwrite: t.overwrite,
    sync_delete: t.sync_delete,
    flatten: t.flatten,
    video_extensions: t.video_extensions || ['.mp4', '.mkv']
  }
  showTaskModal.value = true
}

// 保存任务
async function saveTask() {
  if (!taskForm.value.source_dir || !taskForm.value.target_dir) {
    alert('请填写源目录和目标目录')
    return
  }

  const isEdit = !!editingTaskId.value
  const url = isEdit ? `/api/tasks/${editingTaskId.value}` : '/api/tasks'
  const method = isEdit ? 'PUT' : 'POST'

  try {
    const res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(taskForm.value)
    })
    const json = await res.json()
    if (json.success) {
      showTaskModal.value = false
      loadTasks()
    } else {
      alert('保存失败: ' + json.error)
    }
  } catch (e) {
    alert('请求失败: ' + e.message)
  }
}

// 删除任务
async function deleteTask(id) {
  if (!confirm('确定删除该任务吗？不会影响本地已生成的 STRM 文件')) return
  try {
    const res = await fetch(`/api/tasks/${id}`, { method: 'DELETE' })
    const json = await res.json()
    if (json.success) {
      loadTasks()
    }
  } catch (e) {
    alert('删除失败: ' + e.message)
  }
}

// 手动触发任务
async function triggerTask(id) {
  try {
    const res = await fetch(`/api/tasks/${id}/run`, { method: 'POST' })
    const json = await res.json()
    if (!json.success) {
      alert('启动任务失败: ' + json.error)
    } else {
      // 标记为 running
      const t = tasks.value.find(x => x.id === id)
      if (t) {
        if (!t.runtime) t.runtime = {}
        t.runtime.status = 'running'
      }
    }
  } catch (e) {
    alert('请求失败: ' + e.message)
  }
}

function openLogs(id) {
  const el = document.querySelector('.logs-section')
  if (el) el.scrollIntoView({ behavior: 'smooth' })
}

// 连接 SSE 实时接收进度和日志
function initSSE() {
  const eventSource = new EventSource('/api/tasks/events')
  
  eventSource.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data)
      if (data.type === 'Log') {
        logs.value.push(data.data)
        if (logs.value.length > 500) logs.value.shift()
        nextTick(() => {
          if (terminalBox.value) {
            terminalBox.value.scrollTop = terminalBox.value.scrollHeight
          }
        })
      } else if (data.type === 'StatusChanged') {
        const payload = data.data
        const t = tasks.value.find(x => x.id === payload.task_id)
        if (t) {
          if (!t.runtime) t.runtime = {}
          t.runtime.status = payload.status
          if (payload.last_stats) t.runtime.last_stats = payload.last_stats
          if (payload.error) t.runtime.last_error = payload.error
          t.runtime.last_run = new Date().toISOString()
        }
      }
    } catch (e) {
      console.error('解析 SSE 数据错误:', e)
    }
  }

  eventSource.onerror = () => {
    // 自动重连
  }
}

function statusText(s) {
  switch (s) {
    case 'running': return '⏳ 正在扫描生成'
    case 'success': return '✓ 最近成功'
    case 'failed': return '✗ 最近失败'
    default: return '● 待命就绪'
  }
}

function formatDate(d) {
  if (!d) return ''
  const dt = new Date(d)
  return dt.toLocaleString('zh-CN', { hour12: false })
}

onMounted(async () => {
  await loadConfig()
  await loadTasks()
  testCurrentAlist()
  initSSE()
})
</script>

<style scoped>
.app-layout {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
}

.header {
  background: #1e293b;
  border-bottom: 1px solid #334155;
  padding: 16px 24px;
}

.header-inner {
  max-width: 1200px;
  margin: 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.logo {
  display: flex;
  align-items: center;
  gap: 12px;
}

.logo-icon {
  font-size: 32px;
}

.logo-title {
  font-size: 20px;
  font-weight: 700;
  color: #f8fafc;
}

.logo-sub {
  font-size: 12px;
  color: #94a3b8;
}

.header-actions {
  display: flex;
  gap: 12px;
}

.main-content {
  flex: 1;
  max-width: 1200px;
  width: 100%;
  margin: 0 auto;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.status-banner {
  background: #1e293b;
  border: 1px solid #334155;
  border-radius: 8px;
  padding: 12px 18px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 14px;
}

.status-banner.connected {
  border-color: #059669;
  background: rgba(6, 78, 59, 0.2);
}

.status-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #ef4444;
}

.status-banner.connected .status-dot {
  background: #10b981;
}

.status-sep {
  color: #475569;
}

.status-url {
  color: #94a3b8;
  font-family: monospace;
}

.btn-text {
  background: transparent;
  color: #60a5fa;
  font-size: 13px;
  text-decoration: underline;
}

.section-header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 16px;
}

.section-header h2 {
  font-size: 18px;
  font-weight: 600;
}

.task-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(360px, 1fr));
  gap: 20px;
}

.task-card {
  background: #1e293b;
  border: 1px solid #334155;
  border-radius: 10px;
  padding: 18px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1);
}

.task-card-header {
  display: flex;
  flex-direction: column;
  gap: 6px;
  border-bottom: 1px solid #334155;
  padding-bottom: 12px;
}

.task-title-wrap {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.task-title {
  font-size: 16px;
  font-weight: 600;
}

.task-mode-tag {
  font-size: 12px;
  color: #60a5fa;
  font-weight: 500;
}

.task-card-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-size: 13px;
}

.info-row {
  display: flex;
  justify-content: space-between;
  gap: 12px;
}

.info-label {
  color: #94a3b8;
  white-space: nowrap;
}

.info-val {
  text-align: right;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.code-text {
  font-family: monospace;
  background: #0f172a;
  padding: 1px 6px;
  border-radius: 4px;
}

.run-summary {
  margin-top: 6px;
  padding: 8px 10px;
  background: #0f172a;
  border-radius: 6px;
  border-left: 3px solid #3b82f6;
  font-size: 12px;
}

.summary-title {
  color: #94a3b8;
  margin-bottom: 4px;
}

.summary-stats {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.stat-tag {
  background: #334155;
  padding: 1px 6px;
  border-radius: 4px;
}

.stat-tag.success {
  color: #6ee7b7;
}

.stat-tag.danger {
  color: #fca5a5;
}

.summary-error {
  color: #f87171;
  margin-top: 4px;
}

.task-card-footer {
  margin-top: auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding-top: 12px;
  border-top: 1px solid #334155;
}

.footer-sub-actions {
  display: flex;
  gap: 6px;
}

.empty-state {
  text-align: center;
  padding: 48px;
  background: #1e293b;
  border: 1px dashed #475569;
  border-radius: 10px;
  color: #94a3b8;
}

.empty-icon {
  font-size: 40px;
  margin-bottom: 12px;
}

.logs-section {
  background: #1e293b;
  border: 1px solid #334155;
  border-radius: 10px;
  padding: 18px;
}

.log-header-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.live-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #10b981;
  box-shadow: 0 0 8px #10b981;
}

.terminal-empty {
  color: #64748b;
  text-align: center;
  padding: 24px;
}

/* 模态框样式 */
.modal-header {
  padding: 16px 20px;
  border-bottom: 1px solid #334155;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.modal-close {
  background: transparent;
  color: #94a3b8;
  font-size: 18px;
}

.modal-body {
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.modal-footer {
  padding: 16px 20px;
  border-top: 1px solid #334155;
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

.form-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-group label {
  font-size: 13px;
  font-weight: 500;
  color: #e2e8f0;
}

.form-group small {
  font-size: 12px;
  color: #94a3b8;
}

.form-row {
  display: flex;
  gap: 16px;
}

.flex-1 {
  flex: 1;
}

.form-check-group {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-top: 6px;
  background: #0f172a;
  padding: 12px;
  border-radius: 6px;
}

.check-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: #cbd5e1;
  cursor: pointer;
}

.test-result-box {
  padding: 10px;
  border-radius: 6px;
  font-size: 13px;
  background: #7f1d1d;
  color: #fca5a5;
}

.test-result-box.success {
  background: #064e3b;
  color: #6ee7b7;
}
</style>
